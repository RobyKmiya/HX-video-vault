//! `folder` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

/// 主页层级浏览：列出某层下的 auto 子文件夹卡。
/// parent = None 取顶层（名字里没有 › 的单段夹）；Some(路径) 取以「路径 › 」开头的一层。
/// 封面拼贴素材取夹内前 4 个成员（按位次）的已有图片：自选封面 > 中帧缓存 > 接触表，
/// 现场不生成任何图（点进文件夹后视频卡片的懒加载会逐步把缓存填满，下一轮浏览自然有图）。
pub fn list_folder_children(
    conn: &Connection,
    root: &Path,
    parent: Option<&str>,
) -> Result<Vec<crate::models::FolderInfo>, String> {
    const SEP: &str = " › ";
    let (like_kw, bind): (&str, String) = match parent {
        None => ("NOT LIKE", format!("%{}%", SEP)),
        Some(p) => ("LIKE", format!("%{}{}%", escape_like(p), escape_like(SEP))),
    };
    let sql = format!(
        "SELECT c.id, c.name, COUNT(cv.video_id), COALESCE(MAX(v.added_at), 0)
         FROM collections c
         LEFT JOIN collection_videos cv ON cv.collection_id = c.id
         LEFT JOIN videos v ON v.id = cv.video_id
         WHERE c.kind='auto' AND c.deleted_at=0 AND c.name {like_kw} ? ESCAPE '\\'
         GROUP BY c.id ORDER BY c.name COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![bind], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut member_stmt = conn
        .prepare("SELECT video_id FROM collection_videos WHERE collection_id=?1 ORDER BY position LIMIT 4")
        .map_err(|e| e.to_string())?;
    let cover_dir = LibraryContext::vault_dir(root).join("cache").join("cover");

    let mut out = Vec::new();
    for (id, name, count, latest_added) in rows.filter_map(Result::ok) {
        let leaf = match parent {
            None => name.clone(),
            Some(p) => name[p.len() + SEP.len()..].to_string(),
        };
        let mut covers: Vec<String> = Vec::new();
        let member_ids: Vec<i64> = member_stmt
            .query_map(params![id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        for vid in member_ids {
            if covers.len() >= 4 {
                break;
            }
            let candidates = [
                get_cover_path(conn, vid).ok().flatten(),
                Some(cover_dir.join(format!("mid-{vid}.jpg")).to_string_lossy().to_string()),
            ];
            for c in candidates.into_iter().flatten() {
                if covers.len() < 4 && Path::new(&c).is_file() {
                    covers.push(c);
                    break;
                }
            }
            if covers.len() < 4 {
                if let Ok(item) = get_video(conn, vid) {
                    if !item.contact_sheet.is_empty() && Path::new(&item.contact_sheet).is_file() {
                        covers.push(item.contact_sheet);
                    }
                }
            }
        }
        out.push(crate::models::FolderInfo { id, name, leaf, count, latest_added, covers });
    }
    Ok(out)
}
/// 扫描后同步「磁盘文件夹 → 自动收藏夹」：
/// 库根下的每层子目录镜像成 kind='auto' 的收藏夹（名 = 相对路径以 › 连接，分组 = 首段），
/// 视频/压缩包条目按所在目录自动归位；文件移走或删掉的旧链接会被清掉。
/// 根目录直接放的文件不归类（无文件夹可镜像）。手动收藏夹（custom/system）不受影响。
/// 返回本次同步的文件夹数。
pub fn sync_folder_collections(
    conn: &Connection,
    root: &Path,
    seen_paths: &[String],
) -> Result<usize, String> {
    use std::collections::{HashMap, HashSet};

    let root_str = root.to_string_lossy().replace('/', "\\");
    let seen: HashSet<&String> = seen_paths.iter().collect();

    // 本次在盘上的视频 id → 所在相对目录（空串 = 根目录，跳过）
    let mut dir_ids: HashMap<String, Vec<i64>> = HashMap::new();
    let mut dir_paths: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, path FROM videos")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for (vid, path) in rows.filter_map(Result::ok) {
            if !seen.contains(&path) {
                continue;
            }
            let rel = path
                .strip_prefix(&root_str)
                .map(|rest| rest.trim_start_matches(['\\', '/']))
                .unwrap_or("");
            let Some(idx) = rel.rfind(['\\', '/']) else { continue }; // 根目录直放：无文件夹
            let rel_dir = &rel[..idx];
            dir_ids.entry(rel_dir.to_string()).or_default().push(vid);
            dir_paths.entry(rel_dir.to_string()).or_default().push(path);
        }
    }

    let mut synced = 0usize;
    for (rel_dir, _ids) in &dir_ids {
        let name = rel_dir.replace(['\\', '/'], " › ");
        let group = name.split(" › ").next().unwrap_or("").to_string();
        conn.execute(
            "INSERT OR IGNORE INTO collections (name, kind, group_name) VALUES (?1, 'auto', ?2)",
            params![name, group],
        )
        .map_err(|e| e.to_string())?;
        let cid: i64 = conn
            .query_row(
                "SELECT id FROM collections WHERE name=?1 AND kind='auto'",
                params![name],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let paths_json = serde_json::to_string(&dir_paths[rel_dir]).map_err(|e| e.to_string())?;

        // 文件已移走/删除的旧链接清掉
        conn.execute(
            "DELETE FROM collection_videos
             WHERE collection_id=?1
               AND video_id NOT IN (SELECT id FROM videos WHERE path IN (SELECT value FROM json_each(?2)))",
            params![cid, paths_json],
        )
        .map_err(|e| e.to_string())?;

        // 缺的链接补上，位次排到当前夹尾
        conn.execute(
            "INSERT OR IGNORE INTO collection_videos (collection_id, video_id, added_at, position)
             SELECT ?1, v.id, ?2,
                    COALESCE((SELECT MAX(position)+1 FROM collection_videos WHERE collection_id=?1), 0)
                      + ROW_NUMBER() OVER (ORDER BY v.id) - 1
             FROM videos v
             WHERE v.path IN (SELECT value FROM json_each(?3))
               AND v.id NOT IN (SELECT video_id FROM collection_videos WHERE collection_id=?1)",
            params![cid, now_secs(), paths_json],
        )
        .map_err(|e| e.to_string())?;
        synced += 1;
    }

    // 已清空/已消失的文件夹：目录里已经没有视频 → 整夹清空并删除，
    // 否则残留链接永远挂在 side 栏（文件夹是唯一事实源，夹随盘走）
    let occupied: HashSet<String> = dir_ids
        .keys()
        .map(|d| d.replace(['\\', '/'], " › "))
        .collect();
    let mut stmt = conn
        .prepare("SELECT id, name FROM collections WHERE kind='auto'")
        .map_err(|e| e.to_string())?;
    let autos: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    for (cid, name) in autos {
        if !occupied.contains(&name) {
            conn.execute("DELETE FROM collection_videos WHERE collection_id=?1", params![cid])
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM collections WHERE id=?1", params![cid])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(synced)
}
/// 夹内全部成员 id（按位次）。
pub fn collection_member_ids(conn: &Connection, collection_id: i64) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT video_id FROM collection_videos WHERE collection_id=?1 ORDER BY position")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![collection_id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

// ---------- 应用内文件编辑：改名 / 移动后的库内同步 ----------
