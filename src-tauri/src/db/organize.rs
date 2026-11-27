//! 库的物理整理：把视频收进 1、2、3… 号文件夹（每夹 200），并维护导入/位置计数。
//!
//! 为什么要有这个模块：浏览器/下载器常把新视频直接丢在库根或老区间夹里，一个夹塞几百条
//! 会让首屏扫描、抽帧、文件系统遍历一起变慢。这里按「导入顺序」重新装箱，夹名改成纯数字，
//! 用户一眼能看出「第几号夹、夹内第几条」。

use serde::Serialize;

use crate::media;

use super::*;

/// 单夹容量：200 条。
pub const FOLDER_CAP: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct OrganizeReport {
    /// 真正移动过的文件数
    pub moved: usize,
    /// 整理后一共有多少个编号夹
    pub folders: usize,
    /// 因为目标重名/移动失败而留在原地的条目
    pub skipped: Vec<String>,
}

/// 目录名 → 夹编号：纯数字直接用；旧的 `0-199` 区间按 200 一段折成 1、2、3…
pub fn folder_number_of(name: &str) -> Option<i64> {
    let t = name.trim();
    if let Ok(n) = t.parse::<i64>() {
        return if n > 0 { Some(n) } else { None };
    }
    let (a, _) = t.split_once('-')?;
    let start: i64 = a.trim().parse().ok()?;
    Some(start.div_euclid(FOLDER_CAP as i64) + 1)
}

/// 从现有路径解析「第几号夹」。
fn folder_no_from_path(path: &str) -> Option<i64> {
    let parent = Path::new(path).parent()?.file_name()?.to_string_lossy().to_string();
    folder_number_of(&parent)
}

/// 按导入顺序回填「第几号夹 / 夹内第几条」。迁移时用，不动文件。
pub fn backfill_positions(conn: &Connection) -> Result<(), String> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, path FROM videos ORDER BY import_seq ASC, id ASC")
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        mapped.filter_map(Result::ok).collect()
    };
    let mut counters: HashMap<i64, i64> = HashMap::new();
    for (id, path) in rows {
        let Some(folder) = folder_no_from_path(&path) else { continue };
        let slot = counters.entry(folder).or_insert(0);
        *slot += 1;
        conn.execute(
            "UPDATE videos SET folder_no=?2, slot_no=?3 WHERE id=?1",
            params![id, folder, *slot],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 把所有活跃视频按导入顺序重新装箱到 `1..N` 号夹（每夹 `FOLDER_CAP` 条）。
///
/// 规则：
/// - 不改文件名（文件名是用户辨认与去重的依据）；目标夹里已经有同名文件就**跳过并报告**，不重命名；
/// - 只动活跃的普通文件条目（压缩包内条目 entry_index>0 没有独立文件，不参与）；
/// - 同一卷内 `fs::rename` 是元数据操作，1900 个文件也是秒级；
/// - 移动后走 `apply_path_change` 更新记录与文件夹镜像，保留标签/观看历史/封面。
pub fn organize_numbered(lib: &LibraryContext, cap: usize) -> Result<OrganizeReport, String> {
    let cap = cap.max(1);
    let root = &lib.root;
    let rows: Vec<(i64, String)> = {
        let mut stmt = lib
            .conn
            .prepare(
                "SELECT id, path FROM videos
                 WHERE trashed=0 AND entry_index=0
                 ORDER BY import_seq ASC, id ASC",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        mapped.filter_map(Result::ok).collect()
    };

    // 先把库内混写的 `F:/x\y` 统一成 `F://x//y`，否则和扫描出来的路径对不上
    let _ = lib
        .conn
        .execute("UPDATE videos SET path = REPLACE(path, '/', '\\') WHERE path LIKE '%/%'", []);

    let mut moved = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let root = &PathBuf::from(root.to_string_lossy().replace('/', "\\"));
    let mut folders = 0usize;
    for (idx, (id, path)) in rows.iter().enumerate() {
        let folder = (idx / cap + 1) as i64;
        folders = folders.max(folder as usize);
        let slot = (idx % cap + 1) as i64;
        let src = PathBuf::from(path);
        let Some(name) = src.file_name().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        if !src.is_file() {
            continue; // 幽灵行交给 purge_missing_trashed
        }
        let dir = root.join(folder.to_string());
        // 统一反斜杠：库里的路径必须和扫描 walk 出来的写法一致，
        // 否则 `WHERE path=?` 匹配不上，会把老记录判成丢失、再插一条新记录。
        let dst = PathBuf::from(dir.join(&name).to_string_lossy().replace('/', "\\"));
        let same = src == dst
            || src
                .to_string_lossy()
                .replace('/', "\\")
                .eq_ignore_ascii_case(&dst.to_string_lossy().replace('/', "\\"));
        if !same {
            if dst.exists() {
                skipped.push(format!("{name}：{} 号夹里已有同名文件，未移动", folder));
                continue;
            }
            fs::create_dir_all(&dir).map_err(|e| format!("无法创建 {folder} 号夹: {e}"))?;
            if let Err(e) = fs::rename(&src, &dst) {
                skipped.push(format!("{name}：移动失败 {e}"));
                continue;
            }
            apply_path_change(&lib.conn, root, *id, &dst)?;
            moved += 1;
        }
        lib.conn
            .execute(
                "UPDATE videos SET folder_no=?2, slot_no=?3 WHERE id=?1",
                params![id, folder, slot],
            )
            .map_err(|e| e.to_string())?;
    }

    // 清掉搬空的老夹（只认「纯数字」或「N-M 区间」命名的目录，别误删用户自建夹）
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().map(|s| s.to_string_lossy().to_string()) else { continue };
            if name.starts_with('.') || folder_number_of(&name).is_none() {
                continue;
            }
            let empty = fs::read_dir(&p)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = fs::remove_dir(&p);
            }
        }
    }

    // 有跳过/重名时编号会漂：最后按「磁盘上的实际夹」重算一遍位置计数，保证唯一且对得上
    backfill_positions(&lib.conn)?;

    Ok(OrganizeReport { moved, folders, skipped })
}

/// 便宜的前置判断：库根下有散落视频，或存在「非编号夹」里还有视频 → 值得整理一次。
/// 启动时先跑这个，避免每次开应用都白跑一遍 1900 次 rename。
pub fn needs_organize(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else { return false };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            if media::is_video(&p) {
                return true;
            }
            continue;
        }
        if !p.is_dir() {
            continue;
        }
        let Some(name) = p.file_name().map(|s| s.to_string_lossy().to_string()) else { continue };
        if name.starts_with('.') || folder_number_of(&name).is_some() {
            continue;
        }
        if let Ok(inner) = fs::read_dir(&p) {
            if inner.flatten().any(|e| media::is_video(&e.path())) {
                return true;
            }
        }
    }
    false
}
