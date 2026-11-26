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
