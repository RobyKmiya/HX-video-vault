//! `files` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

// ---------- 应用内文件编辑：重命名 / 移动 / 定位 ----------

/// 移动目标（对话框用）：库根 + 所有 auto 文件夹（绝对路径直接给后端用）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MoveTarget {
    name: String,
    path: String,
    count: i64,
}

#[tauri::command]
pub(crate) fn move_targets(state: State<'_, AppState>) -> Result<Vec<MoveTarget>, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let mut out = vec![MoveTarget {
        name: "库根".into(),
        path: lib.root.to_string_lossy().to_string(),
        count: 0,
    }];
    for (name, path, count) in db::list_move_targets(&lib.conn, &lib.root)? {
        out.push(MoveTarget { name, path, count });
    }
    Ok(out)
}

/// 重命名（同目录改文件名）：磁盘 rename 是毫秒级，封面缓存按 id 所以不重抽；
/// 返回更新后的整条，前端就地替换，不用重新拉列表。
#[tauri::command]
pub(crate) fn rename_video(state: State<'_, AppState>, id: i64, name: String) -> Result<VideoItem, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let (path, entry_index) = db::video_source(&lib.conn, id)?;
    if entry_index > 0 {
        return Err("压缩包里的条目不能单独改名（改名请直接改压缩包）".into());
    }
    let src = PathBuf::from(&path);
    if !src.is_file() {
        return Err("原文件已经不在原位了，先点「扫描库」".into());
    }
    let new_path = fileops::rename_in_place(&src, &name)?;
    db::apply_path_change(&lib.conn, &lib.root, id, &new_path)?;
    db::get_video(&lib.conn, id)
}

/// 批量移动到库内某个目录：逐条 rename + 只同步这一条的镜像夹，不做全库扫描。
#[tauri::command]
pub(crate) fn move_videos(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    target: String,
) -> Result<usize, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let raw_target = PathBuf::from(target.trim());
    if raw_target.as_os_str().is_empty() {
        return Err("没有选择目标文件夹".into());
    }
    fs::create_dir_all(&raw_target).map_err(|e| format!("无法创建目标文件夹：{e}"))?;
    let root_c = fs::canonicalize(&lib.root).map_err(|e| format!("库目录不可用：{e}"))?;
    let target_c = fs::canonicalize(&raw_target).map_err(|e| format!("目标文件夹不可用：{e}"))?;
    if !target_c.starts_with(&root_c) {
        return Err("只能移动到库目录内的文件夹（库外的文件请用资源管理器）".into());
    }

    let mut moved = 0usize;
    let mut first_err = String::new();
    for id in ids {
        let Ok((path, entry_index)) = db::video_source(&lib.conn, id) else { continue };
        if entry_index > 0 {
            if first_err.is_empty() {
                first_err = "压缩包里的条目不能单独移动".into();
            }
            continue;
        }
        let src = PathBuf::from(&path);
        if !src.is_file() {
            if first_err.is_empty() {
                first_err = "有文件已不在原位（先扫描库）".into();
            }
            continue;
        }
        match fileops::move_to(&src, &target_c) {
            Ok(dest) => {
                db::apply_path_change(&lib.conn, &lib.root, id, &dest)?;
                moved += 1;
            }
            Err(e) => {
                if first_err.is_empty() {
                    first_err = e;
                }
            }
        }
    }
    if moved == 0 && !first_err.is_empty() {
        return Err(first_err);
    }
    Ok(moved)
}

/// 在资源管理器里选中该文件（不打开播放器）。
#[tauri::command]
pub(crate) fn reveal_video(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let (path, entry_index) = db::video_source(&lib.conn, id)?;
    if entry_index > 0 {
        return Err("压缩包条目没有独立文件，没法定位".into());
    }
    fileops::reveal(Path::new(&path))
}

// ---------- 彻底删除（回收站二次确认后的根除） ----------

/// 彻底删除回收站里的条目：**连磁盘文件一起删**，删完从库里消失且不会被扫描捡回。
///
/// 安全约束（这条命令不可逆，宁严勿松）：
/// 1. 只接受 `trashed=1` 的条目，不在回收站的直接拒绝（防止误调 API 删活跃视频）；
/// 2. 只删库根目录内的文件（canonicalize 后前缀校验），库外路径只清记录不动文件；
/// 3. 压缩包内条目（entry_index>0）不单独删，提示去删整个压缩包；
/// 4. 顺手清掉该 id 的封面/中帧/接触表缓存，避免残留垃圾。
#[tauri::command]
pub(crate) async fn purge_videos(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<models::PurgeReport, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = state.lock()?;
        let lib = library(&ctx)?;
        purge_videos_in(lib, &ids)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `purge_videos` 的实现主体（不碰 AppState，便于单测）。约束见上方注释。
pub(crate) fn purge_videos_in(
    lib: &LibraryContext,
    ids: &[i64],
) -> Result<models::PurgeReport, String> {
    let root_c = fs::canonicalize(&lib.root).map_err(|e| format!("库目录不可用：{e}"))?;
    let cache = LibraryContext::vault_dir(&lib.root).join("cache");
    let mut deleted = 0usize;
    let mut bytes = 0i64;
    let mut failed: Vec<String> = Vec::new();

    for &id in ids {
        let Ok(item) = db::get_video(&lib.conn, id) else {
            failed.push(format!("#{id}: 记录不存在"));
            continue;
        };
        if !item.trashed {
            failed.push(format!("#{id}「{}」不在回收站，拒绝彻底删除", item.file_name));
            continue;
        }
        let Ok((path, entry_index)) = db::video_source(&lib.conn, id) else {
            failed.push(format!("#{id}: 找不到源路径"));
            continue;
        };
        if entry_index > 0 {
            failed.push(format!("#{id}「{}」是压缩包内条目，请删整个压缩包", item.file_name));
            continue;
        }
        let src = PathBuf::from(&path);
        match fs::canonicalize(&src) {
            Ok(p) if p.starts_with(&root_c) => {
                if p.is_file() {
                    if let Err(e) = fs::remove_file(&p) {
                        failed.push(format!("#{id}「{}」删文件失败：{e}", item.file_name));
                        continue;
                    }
                    bytes += item.size_bytes;
                }
            }
            Ok(_) => {
                failed.push(format!("#{id}「{}」不在库根内，已跳过（只删库外文件不安全）", item.file_name));
                continue;
            }
            // 文件已经不在磁盘：直接把记录清掉，不算失败
            Err(_) => {}
        }
        // 缓存图一并清掉：自选封面 / 中帧 / 接触表
        for name in [
            format!("cover-{id}.jpg"),
            format!("cover-{id}.png"),
            format!("mid-{id}.jpg"),
        ] {
            let _ = fs::remove_file(cache.join("cover").join(name));
        }
        let _ = fs::remove_file(cache.join("contacts").join(format!("{id}.jpg")));
        if let Err(e) = db::delete_video_row(&lib.conn, id) {
            failed.push(format!("#{id}: 清库记录失败：{e}"));
            continue;
        }
        deleted += 1;
    }
    Ok(models::PurgeReport { deleted, bytes, failed })
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn temp_lib() -> (PathBuf, LibraryContext) {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("vv-purge-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ctx = db::open_library(&root).expect("临时库应能创建");
        (root, ctx)
    }

    fn insert(conn: &rusqlite::Connection, path: &str) -> i64 {
        let rec = db::VideoRecord {
            path: path.into(),
            file_name: Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            container: "mp4".into(),
            size_bytes: 4096,
            mtime: 100,
            duration_sec: 10.0,
            width: 1920,
            height: 1080,
            codec_video: "h264".into(),
            codec_audio: "aac".into(),
            entry_index: 0,
            entry_count: 1,
        };
        db::upsert_video(conn, &rec).unwrap();
        conn.query_row("SELECT id FROM videos WHERE path=?1", rusqlite::params![path], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn purge_deletes_file_and_row_for_trashed_only() {
        let (root, ctx) = temp_lib();
        let inside = root.join("0-199").join("gone.mp4");
        fs::create_dir_all(inside.parent().unwrap()).unwrap();
        fs::write(&inside, b"fake-video").unwrap();
        let id = insert(&ctx.conn, &inside.to_string_lossy());

        // 不在回收站 → 拒绝，文件保留
        let r = purge_videos_in(&ctx, &[id]).unwrap();
        assert_eq!(r.deleted, 0);
        assert!(inside.is_file(), "活跃视频不能被彻底删除");
        assert!(!r.failed.is_empty());

        // 进回收站后 → 连文件一起删
        db::set_trashed(&ctx.conn, id, true).unwrap();
        let r = purge_videos_in(&ctx, &[id]).unwrap();
        assert_eq!(r.deleted, 1);
        assert_eq!(r.bytes, 4096);
        assert!(!inside.exists(), "磁盘文件应被删掉");
        let left: i64 = ctx.conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "库记录应被清掉");

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn purge_refuses_files_outside_library_root() {
        let (root, ctx) = temp_lib();
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let outside = std::env::temp_dir().join(format!("vv-outside-{}-{n}.mp4", std::process::id()));
        fs::write(&outside, b"x").unwrap();
        let id = insert(&ctx.conn, &outside.to_string_lossy());
        db::set_trashed(&ctx.conn, id, true).unwrap();

        let r = purge_videos_in(&ctx, &[id]).unwrap();
        assert_eq!(r.deleted, 0, "库外文件不能删");
        assert!(outside.is_file(), "库外文件必须原样保留");
        let left: i64 = ctx.conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1, "拒绝时记录也要保留");

        let _ = fs::remove_file(&outside);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn purge_cleans_row_when_file_already_missing() {
        let (root, ctx) = temp_lib();
        let ghost = root.join("0-199").join("ghost.mp4");
        let id = insert(&ctx.conn, &ghost.to_string_lossy());
        db::set_trashed(&ctx.conn, id, true).unwrap();

        let r = purge_videos_in(&ctx, &[id]).unwrap();
        assert_eq!(r.deleted, 1, "文件不在时也应该能清掉记录");
        assert_eq!(r.bytes, 0, "没删到文件不算释放空间");

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }
}
