//! `collections` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

#[tauri::command]
pub(crate) fn list_tags(state: State<'_, AppState>) -> Result<Vec<Tag>, String> {
    let ctx = state.lock()?;
    db::list_tags(&library(&ctx)?.conn)
}

#[tauri::command]
pub(crate) fn create_tag(state: State<'_, AppState>, name: String, category: String, weight: i64) -> Result<i64, String> {
    let ctx = state.lock()?;
    db::create_tag(&library(&ctx)?.conn, &name, &category, weight)
}

#[tauri::command]
pub(crate) fn update_tag(state: State<'_, AppState>, id: i64, name: String, category: String, weight: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    db::update_tag(&library(&ctx)?.conn, id, &name, &category, weight)
}

#[tauri::command]
pub(crate) fn set_tag_video(state: State<'_, AppState>, video_id: i64, tag_id: i64, add: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    db::set_tag_video(&library(&ctx)?.conn, video_id, tag_id, add)
}

#[tauri::command]
pub(crate) fn bulk_tag(state: State<'_, AppState>, video_ids: Vec<i64>, tag_ids: Vec<i64>, add: bool) -> Result<usize, String> {
    let ctx = state.lock()?;
    db::bulk_tag(&library(&ctx)?.conn, &video_ids, &tag_ids, add)
}

#[tauri::command]
pub(crate) fn list_collections(state: State<'_, AppState>, include_deleted: Option<bool>) -> Result<Vec<Collection>, String> {
    let ctx = state.lock()?;
    db::list_collections(&library(&ctx)?.conn, include_deleted.unwrap_or(false))
}

#[tauri::command]
pub(crate) fn create_collection(state: State<'_, AppState>, name: String, group: Option<String>) -> Result<i64, String> {
    let ctx = state.lock()?;
    db::create_collection(&library(&ctx)?.conn, &name, group.as_deref().unwrap_or(""))
}

#[tauri::command]
pub(crate) fn rename_collection(state: State<'_, AppState>, id: i64, name: String) -> Result<(), String> {
    let ctx = state.lock()?;
    db::rename_collection(&library(&ctx)?.conn, id, &name)
}

#[tauri::command]
pub(crate) fn set_collection_group(state: State<'_, AppState>, id: i64, group: String) -> Result<(), String> {
    let ctx = state.lock()?;
    db::set_collection_group(&library(&ctx)?.conn, id, &group)
}

#[tauri::command]
pub(crate) fn delete_collection(state: State<'_, AppState>, id: i64, purge: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    if purge {
        db::purge_collection(&lib.conn, id)
    } else {
        db::soft_delete_collection(&lib.conn, id)
    }
}

#[tauri::command]
pub(crate) fn restore_collection(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    db::restore_collection(&library(&ctx)?.conn, id)
}

#[tauri::command]
pub(crate) fn set_collection_video(
    state: State<'_, AppState>,
    collection_id: Option<i64>,
    name: Option<String>,
    video_id: i64,
    add: bool,
) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    // 兼容两种入口：详情页按 id，旧代码/脚本按名字
    let id = match collection_id {
        Some(id) => id,
        None => db::collection_id_by_name(&lib.conn, name.as_deref().ok_or("缺少目录 id")?)?,
    };
    db::set_collection_video(&lib.conn, id, video_id, add)
}
/// 侧栏收藏夹拖拽排序：前端给出拖完后的完整 id 顺序。
#[tauri::command]
pub(crate) fn reorder_collections(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::reorder_collections(&lib.conn, &ids)
}

/// 收藏夹内视频拖拽排序：前端给出可见子集的新顺序，未在子集里的成员整体后移保序。
#[tauri::command]
pub(crate) fn reorder_collection_videos(
    state: State<'_, AppState>,
    collection_id: i64,
    video_ids: Vec<i64>,
) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::reorder_collection_videos(&lib.conn, collection_id, &video_ids)
}
