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

/// 下载请求：媒体直链 + 作品页信息（作品页用来登记 hook：原链接/作者/备注/标签）。
#[derive(Debug, serde::Deserialize)]
pub(crate) struct DownloadRequest {
    pub url: String,
    #[serde(default)]
    pub page_url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct EnqueueResult {
    pub id: i64,
    pub duplicate: bool,
    pub message: String,
}

/// 统一的「丢链接下载」入口：GUI 与油猴桥走同一条链路。
///
/// 顺序：作品页查重 → 登记网页元数据（hook）→ 入队（同 URL 去重）→ 起队列。
/// 下载完成后由 downloader::run_queue 直接入库、挂元数据、按导入规则收进编号夹。
#[tauri::command]
pub(crate) fn enqueue_download(
    state: State<'_, AppState>,
    app: AppHandle,
    req: DownloadRequest,
) -> Result<EnqueueResult, String> {
    if req.url.trim().is_empty() {
        return Err("链接不能为空".into());
    }
    let root = state.library_path()?;
    let (id, message) = {
        let ctx = state.lock()?;
        let lib = library(&ctx)?;
        // 同一个作品页已经在库里 → 不重复下载，直接说清楚它在哪
        if !req.page_url.is_empty() {
            if let Some(v) = db::find_by_url(&lib.conn, &req.page_url)? {
                let pos = if v.folder_no > 0 {
                    format!("{} 号夹 第 {} 条", v.folder_no, v.slot_no)
                } else {
                    "待整理".to_string()
                };
                return Ok(EnqueueResult {
                    id: 0,
                    duplicate: true,
                    message: format!("已在库中：{}（{}）", v.file_name, pos),
                });
            }
        }
        // 网页元数据先登记：落盘入库后 apply_pending_hooks 会挂上
        // 原链接 / 作者 / 备注 / 标签（解析标签 + 用户标签合并）
        if !req.page_url.trim().is_empty() && !req.title.trim().is_empty() {
            let tags = downloader::merge_tag_text(&[], &req.tags);
            let _ = db::submit_hook(
                &lib.conn,
                req.page_url.trim(),
                req.title.trim(),
                &req.author,
                &req.site,
                &tags,
                &req.note,
            );
        }
        let id = db::create_download(&lib.conn, &req.url, &req.title, &req.site, "", &req.tags)?;
        (id, "已加入下载队列，完成后自动入库打标".to_string())
    };
    downloader::run_queue(app, root, 50);
    Ok(EnqueueResult { id, duplicate: false, message })
}

/// 清空已完成/已取消的任务记录（只动队列，不碰视频文件）。
#[tauri::command]
pub(crate) fn clear_done_downloads(state: State<'_, AppState>) -> Result<usize, String> {
    let ctx = state.lock()?;
    db::clear_done_downloads(&library(&ctx)?.conn)
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
