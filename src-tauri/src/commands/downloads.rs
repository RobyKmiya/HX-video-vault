//! `downloads` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

// ---------- 下载器 ----------

#[tauri::command]
pub(crate) async fn resolve_link(url: String) -> Result<ResolveReport, String> {
    tauri::async_runtime::spawn_blocking(move || downloader::resolve(&url))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) fn list_downloads(state: State<'_, AppState>) -> Result<Vec<DownloadTask>, String> {
    let ctx = state.lock()?;
    db::list_downloads(&library(&ctx)?.conn, 200)
}

#[tauri::command]
pub(crate) fn add_download(
    state: State<'_, AppState>,
    url: String,
    title: String,
    site: String,
    target_path: String,
    tags: String,
) -> Result<i64, String> {
    let ctx = state.lock()?;
    db::create_download(&library(&ctx)?.conn, &url, &title, &site, &target_path, &tags)
}

#[tauri::command]
pub(crate) fn remove_download(state: State<'_, AppState>, id: i64, delete_file: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    if delete_file {
        if let Ok(task) = db::get_download(&lib.conn, id) {
            if !task.target_path.is_empty() {
                let p = PathBuf::from(&task.target_path);
                let partial = p.with_extension(format!("{}.part", p.extension().and_then(|s| s.to_str()).unwrap_or("bin")));
                for f in [p, partial] {
                    if f.is_file() { let _ = fs::remove_file(f); }
                }
            }
        }
    }
    db::delete_download(&lib.conn, id)
}

/// 启动下载队列（前端在启动与「联网后续跑」时调用）。
#[tauri::command]
pub(crate) fn start_downloads(state: State<'_, AppState>, app: AppHandle) -> Result<usize, String> {
    let root = state.library_path()?;
    {
        let ctx = state.lock()?;
        let n = db::requeue_interrupted(&library(&ctx)?.conn)?;
        if n > 0 {
            let _ = app.emit("download-progress", serde_json::json!({"event": "requeued", "count": n}));
        }
    }
    downloader::run_queue(app, root, 50);
    Ok(1)
}
