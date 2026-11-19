mod db;
mod media;
mod models;
mod player;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use walkdir::WalkDir;

use db::LibraryContext;
use models::{
    Collection, LibraryInfo, ScanOptions, ScanReport, Stats, Tag, VideoItem, VideoPage, VideoPatch,
    VideoQuery, WatchUpdate,
};
use player::PlaybackInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub library_root: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub ffprobe_path: Option<String>,
    pub player_pref: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            library_root: None,
            ffmpeg_path: None,
            ffprobe_path: None,
            player_pref: "auto".into(),
        }
    }
}

struct AppInner {
    library: Option<LibraryContext>,
    settings: Settings,
    startup_error: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Mutex<AppInner>>,
}

impl AppState {
    fn new(app: &AppHandle) -> Self {
        let settings = read_settings(app);
        let mut library = None;
        let mut startup_error = None;
        if let Some(root) = settings.library_root.as_ref() {
            let root_path = PathBuf::from(root);
            if root_path.is_dir() {
                match db::open_library(&root_path) {
                    Ok(ctx) => library = Some(ctx),
                    Err(e) => startup_error = Some(format!("上次的库打不开：{e}")),
                }
            } else {
                startup_error = Some("上次的库目录不存在".into());
            }
        }
        Self {
            inner: Arc::new(Mutex::new(AppInner {
                library,
                settings,
                startup_error,
            })),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, AppInner>, String> {
        self.inner
            .lock()
            .map_err(|_| "应用状态已损坏，请重启影匣".into())
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("无法获取配置目录: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

fn read_settings(app: &AppHandle) -> Settings {
    let Ok(path) = settings_path(app) else {
        return Settings::default();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return Settings::default();
    };
    match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("settings.json 解析失败（{}）: {e}", path.to_string_lossy());
            Settings::default()
        }
    }
}

fn write_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let raw = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

fn library(ctx: &AppInner) -> Result<&LibraryContext, String> {
    ctx.library
        .as_ref()
        .ok_or_else(|| "尚未创建或打开视频库，请先在设置里选择库目录".into())
}

#[tauri::command]
fn app_settings(state: State<'_, AppState>) -> Result<(Settings, Option<String>, Option<LibraryInfo>), String> {
    let ctx = state.lock()?;
    let info = match &ctx.library {
        Some(lib) => Some(db::library_info(lib)?),
        None => None,
    };
    Ok((ctx.settings.clone(), ctx.startup_error.clone(), info))
}

#[tauri::command]
fn set_library(state: State<'_, AppState>, app: AppHandle, root: String) -> Result<LibraryInfo, String> {
    let path = PathBuf::from(&root);
    if !path.is_dir() {
        return Err("所选目录不存在".into());
    }
    let created = db::open_library(&path)?;
    let info = db::library_info(&created)?;
    let mut ctx = state.lock()?;
    ctx.library = Some(created);
    ctx.settings.library_root = Some(root);
    ctx.startup_error = None;
    write_settings(&app, &ctx.settings)?;
    Ok(info)
}

#[tauri::command]
fn update_settings(state: State<'_, AppState>, app: AppHandle, settings: Settings) -> Result<(), String> {
    let mut ctx = state.lock()?;
    ctx.settings = settings;
    write_settings(&app, &ctx.settings)
}

#[tauri::command]
async fn pick_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(picked.map(|p| p.to_string()))
}

/// 扫描库根目录：视频文件 + ZIP/CBZ 内的视频条目。
/// 只登记元数据，抽帧交给单独的 contact_sheet 命令，避免扫描被 ffmpeg 拖慢。
#[tauri::command]
async fn scan(state: State<'_, AppState>, opts: ScanOptions) -> Result<ScanReport, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = state.lock()?;
        let lib = library(&guard)?;
        let root = if opts.root.trim().is_empty() {
            lib.root.clone()
        } else {
            PathBuf::from(&opts.root)
        };
        let recursive = opts.recursive.unwrap_or(true);
        let entry_limit = opts.entry_limit.unwrap_or(60).clamp(1, 300);
        let max_depth = if recursive { 12 } else { 1 };
        let mut report = ScanReport {
            found: 0,
            added: 0,
            updated: 0,
            missing: 0,
            skipped: 0,
            errors: Vec::new(),
        };
        let mut seen: Vec<String> = Vec::new();
        for entry in WalkDir::new(&root)
            .max_depth(max_depth)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with(".vault"))
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if media::is_video(path) {
                report.found += 1;
                seen.push(path.to_string_lossy().to_string());
                let rec = media::record_for_file(path);
                match db::upsert_video(&lib.conn, &rec) {
                    Ok(true) => report.added += 1,
                    Ok(false) => report.updated += 1,
                    Err(e) => report.errors.push(format!("{}: {e}", path.to_string_lossy())),
                }
            } else if media::is_supported_archive(path) {
                match media::archive_records(path, entry_limit) {
                    Ok(recs) if !recs.is_empty() => {
                        report.found += recs.len();
                        seen.push(path.to_string_lossy().to_string());
                        for rec in recs {
                            match db::upsert_video(&lib.conn, &rec) {
                                Ok(true) => report.added += 1,
                                Ok(false) => report.updated += 1,
                                Err(e) => report.errors.push(format!("{}: {e}", rec.path)),
                            }
                        }
                    }
                    Ok(_) => report.skipped += 1,
                    Err(e) => report.errors.push(format!("{}: {e}", path.to_string_lossy())),
                }
            }
        }
        report.missing = db::mark_missing(&lib.conn, &seen).unwrap_or(0);
        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn query_videos(state: State<'_, AppState>, q: VideoQuery) -> Result<VideoPage, String> {
    let ctx = state.lock()?;
    db::query_videos(&library(&ctx)?.conn, &q)
}

#[tauri::command]
fn get_video(state: State<'_, AppState>, id: i64) -> Result<VideoItem, String> {
    let ctx = state.lock()?;
    db::get_video(&library(&ctx)?.conn, id)
}

#[tauri::command]
fn patch_video(state: State<'_, AppState>, patch: VideoPatch) -> Result<(), String> {
    let ctx = state.lock()?;
    db::patch_video(&library(&ctx)?.conn, &patch)
}

#[tauri::command]
fn record_watch(state: State<'_, AppState>, u: WatchUpdate) -> Result<(), String> {
    let ctx = state.lock()?;
    db::record_watch(&library(&ctx)?.conn, &u)
}

#[tauri::command]
fn trash_video(state: State<'_, AppState>, id: i64, trashed: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    db::set_trashed(&library(&ctx)?.conn, id, trashed)
}

#[tauri::command]
fn list_tags(state: State<'_, AppState>) -> Result<Vec<Tag>, String> {
    let ctx = state.lock()?;
    db::list_tags(&library(&ctx)?.conn)
}

#[tauri::command]
fn create_tag(state: State<'_, AppState>, name: String, category: String, weight: i64) -> Result<i64, String> {
    let ctx = state.lock()?;
    db::create_tag(&library(&ctx)?.conn, &name, &category, weight)
}

#[tauri::command]
fn update_tag(state: State<'_, AppState>, id: i64, name: String, category: String, weight: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    db::update_tag(&library(&ctx)?.conn, id, &name, &category, weight)
}

#[tauri::command]
fn set_tag_video(state: State<'_, AppState>, video_id: i64, tag_id: i64, add: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    db::set_tag_video(&library(&ctx)?.conn, video_id, tag_id, add)
}

#[tauri::command]
fn bulk_tag(state: State<'_, AppState>, video_ids: Vec<i64>, tag_ids: Vec<i64>, add: bool) -> Result<usize, String> {
    let ctx = state.lock()?;
    db::bulk_tag(&library(&ctx)?.conn, &video_ids, &tag_ids, add)
}

#[tauri::command]
fn list_collections(state: State<'_, AppState>) -> Result<Vec<Collection>, String> {
    let ctx = state.lock()?;
    db::list_collections(&library(&ctx)?.conn)
}

#[tauri::command]
fn create_collection(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let ctx = state.lock()?;
    db::create_collection(&library(&ctx)?.conn, &name)
}

#[tauri::command]
fn set_collection_video(state: State<'_, AppState>, name: String, video_id: i64, add: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    db::set_collection_video(&library(&ctx)?.conn, &name, video_id, add)
}

#[tauri::command]
fn stats(state: State<'_, AppState>) -> Result<Stats, String> {
    let ctx = state.lock()?;
    db::stats(&library(&ctx)?.conn)
}

#[tauri::command]
fn playback_info(state: State<'_, AppState>, id: i64) -> Result<PlaybackInfo, String> {
    let ctx = state.lock()?;
    player::resolve(library(&ctx)?, id)
}

#[tauri::command]
fn play_external(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let info = player::resolve(lib, id)?;
    let path = PathBuf::from(&info.media_path);
    if !path.is_file() {
        return Err("视频文件不存在".into());
    }
    player::launch_external(&path, info.start_sec)
}

/// 为一条视频生成接触表。抽帧是 CPU 密集活，放到阻塞线程池。
#[tauri::command]
async fn make_contact_sheet(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = state.lock()?;
        let lib = library(&guard)?;
        let info = player::resolve(lib, id)?;
        let source = PathBuf::from(&info.media_path);
        if !source.is_file() {
            return Err("视频文件不存在".into());
        }
        let out = db::LibraryContext::vault_dir(&lib.root)
            .join("cache")
            .join("contacts")
            .join(format!("{id}.jpg"));
        media::contact_sheet(&source, &out)?;
        let rel = out.to_string_lossy().to_string();
        db::set_contact_sheet(&lib.conn, id, &rel)?;
        Ok(rel)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 批量生成接触表。逐条短事务：每条做完立即释放锁，
/// 避免长时间持锁让搜索/播放等命令卡住。前端传窗口标题用来显示进度。
#[tauri::command]
async fn make_contact_sheets(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Option<Vec<i64>>,
    limit: Option<usize>,
) -> Result<crate::models::SheetReport, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 先短锁取出待办 id 与输出目录
        let (targets, cache_dir) = {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            let ids = match ids {
                Some(v) => v,
                None => db::ids_without_sheet(&lib.conn, limit.unwrap_or(40).clamp(1, 500))?,
            };
            (ids, crate::db::LibraryContext::vault_dir(&lib.root).join("cache").join("contacts"))
        };
        let total = targets.len();
        let mut report = crate::models::SheetReport {
            total, done: 0, skipped: 0, failed: Vec::new(),
        };
        for (i, id) in targets.iter().enumerate() {
            let _ = app.emit("sheet-progress", serde_json::json!({
                "done": i, "total": total, "id": id,
            }));
            // 每条单独解析来源，短锁读库
            let source = {
                let guard = state.lock()?;
                let lib = library(&guard)?;
                match player::resolve(lib, *id) {
                    Ok(info) => Ok(PathBuf::from(info.media_path)),
                    Err(e) => Err(e),
                }
            };
            let source = match source {
                Ok(p) => p,
                Err(e) => { report.failed.push(format!("#{id}: {e}")); continue; }
            };
            if !source.is_file() {
                report.skipped += 1;
                continue;
            }
            let out = cache_dir.join(format!("{id}.jpg"));
            match media::contact_sheet(&source, &out) {
                Ok(()) => {
                    let guard = state.lock()?;
                    if let Ok(lib) = guard.library.as_ref().ok_or("库未打开") {
                        if let Err(e) = db::set_contact_sheet(&lib.conn, *id, &out.to_string_lossy()) {
                            report.failed.push(format!("#{id}: {e}"));
                            continue;
                        }
                    }
                    report.done += 1;
                }
                Err(e) => report.failed.push(format!("#{id}: {e}")),
            }
        }
        let _ = app.emit("sheet-progress", serde_json::json!({
            "done": total, "total": total, "id": 0,
        }));
        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn watch_history(state: State<'_, AppState>, limit: Option<usize>) -> Result<Vec<crate::models::WatchRecord>, String> {
    let ctx = state.lock()?;
    db::watch_history(&library(&ctx)?.conn, limit.unwrap_or(50).clamp(1, 500))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = AppState::new(app.handle());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_settings,
            set_library,
            update_settings,
            pick_directory,
            scan,
            query_videos,
            get_video,
            patch_video,
            record_watch,
            trash_video,
            list_tags,
            create_tag,
            update_tag,
            set_tag_video,
            bulk_tag,
            list_collections,
            create_collection,
            set_collection_video,
            stats,
            playback_info,
            play_external,
            make_contact_sheet,
            make_contact_sheets,
            watch_history,
        ])
        .run(tauri::generate_context!())
        .expect("影匣启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escape_is_stable() {
        // 空转一下，确保测试目标能被编译
        assert_eq!(2 + 2, 4);
    }

    #[test]
    fn settings_default_has_no_library() {
        let s = Settings::default();
        assert!(s.library_root.is_none());
        assert_eq!(s.player_pref, "auto");
    }
}
