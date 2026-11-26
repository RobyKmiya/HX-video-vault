//! `browse` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

#[tauri::command]
pub(crate) fn query_videos(state: State<'_, AppState>, q: VideoQuery) -> Result<VideoPage, String> {
    let ctx = state.lock()?;
    db::query_videos(&library(&ctx)?.conn, &q)
}

#[tauri::command]
pub(crate) fn get_video(state: State<'_, AppState>, id: i64) -> Result<VideoItem, String> {
    let ctx = state.lock()?;
    db::get_video(&library(&ctx)?.conn, id)
}

#[tauri::command]
pub(crate) fn patch_video(state: State<'_, AppState>, patch: VideoPatch) -> Result<(), String> {
    let ctx = state.lock()?;
    db::patch_video(&library(&ctx)?.conn, &patch)
}

#[tauri::command]
pub(crate) fn record_watch(state: State<'_, AppState>, u: WatchUpdate) -> Result<(), String> {
    let ctx = state.lock()?;
    db::record_watch(&library(&ctx)?.conn, &u)
}

#[tauri::command]
pub(crate) fn trash_video(state: State<'_, AppState>, id: i64, trashed: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    db::set_trashed(&library(&ctx)?.conn, id, trashed)
}
/// 主页层级浏览：某层下的 auto 子文件夹卡（parent=None 取顶层）。
#[tauri::command]
pub(crate) fn list_folder_children(
    state: State<'_, AppState>,
    parent: Option<String>,
) -> Result<Vec<crate::models::FolderInfo>, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::list_folder_children(&lib.conn, &lib.root, parent.as_deref().map(str::trim).filter(|s| !s.is_empty()))
}
