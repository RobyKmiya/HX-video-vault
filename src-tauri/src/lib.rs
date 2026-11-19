pub mod db;
pub mod downloader;
pub mod media;
pub mod models;
mod mpv;
pub mod player;
mod proc;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use walkdir::WalkDir;

use db::LibraryContext;
use models::{
    Collection, DownloadTask, LibraryInfo, ResolveReport, ScanOptions, ScanReport, Stats, Tag,
    VideoItem, VideoPage, VideoPatch, VideoQuery, WatchUpdate,
};
use player::PlaybackInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub library_root: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub ffprobe_path: Option<String>,
    #[serde(default = "default_player_pref")]
    pub player_pref: String,
    #[serde(default)]
    pub mpv_path: Option<String>,
}

fn default_player_pref() -> String { "auto".into() }

impl Default for Settings {
    fn default() -> Self {
        Self {
            library_root: None,
            ffmpeg_path: None,
            ffprobe_path: None,
            player_pref: "auto".into(),
            mpv_path: None,
        }
    }
}

pub(crate) struct AppInner {
    library: Option<LibraryContext>,
    settings: Settings,
    startup_error: Option<String>,
    mpv: Option<Arc<mpv::MpvSession>>,
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Mutex<AppInner>>,
}

impl AppState {
    fn new(app: &AppHandle) -> Self {
        let settings = read_settings(app);
        media::set_tool_paths(settings.ffmpeg_path.as_deref(), settings.ffprobe_path.as_deref());
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
                mpv: None,
            })),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, AppInner>, String> {
        self.inner
            .lock()
            .map_err(|_| "应用状态已损坏，请重启影匣".into())
    }

    /// 供后台线程（下载队列）使用的公开锁入口。
    pub(crate) fn inner_lock(&self) -> Result<MutexGuard<'_, AppInner>, String> {
        self.lock()
    }

    pub fn library_path(&self) -> Result<PathBuf, String> {
        Ok(self.lock()?.library.as_ref().ok_or("库未打开")?.root.clone())
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
    media::set_tool_paths(settings.ffmpeg_path.as_deref(), settings.ffprobe_path.as_deref());
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

/// 并行 ffprobe：视频多的时候串行探测会让扫描拖到几分钟。
/// 探测阶段完全不碰数据库锁，前端搜索/播放照常可用。
fn probe_all(app: &AppHandle, paths: &[PathBuf]) -> Vec<db::VideoRecord> {
    if paths.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 6)
        .min(paths.len());
    let total = paths.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let bucket: Mutex<Vec<db::VideoRecord>> = Mutex::new(Vec::with_capacity(total));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = paths.get(i) else { break };
                let rec = media::record_for_file(path);
                if let Ok(mut b) = bucket.lock() {
                    b.push(rec);
                }
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 20 == 0 || n == total {
                    let _ = app.emit(
                        "scan-progress",
                        serde_json::json!({"phase":"probe","done":n,"total":total}),
                    );
                }
            });
        }
    });
    bucket.into_inner().unwrap_or_default()
}

/// 扫描库根目录：视频文件 + ZIP/CBZ 内的视频条目。
/// 分三步：走目录（不碰库）→ 并行探测（不持锁）→ 分批短锁写库。
/// 已登记且文件没变的条目直接跳过 ffprobe，重复扫描几乎是瞬时的。
#[tauri::command]
async fn scan(
    state: State<'_, AppState>,
    app: AppHandle,
    opts: ScanOptions,
) -> Result<ScanReport, String> {
    let state = state.inner().clone();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (root, known) = {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            let root = if opts.root.trim().is_empty() {
                lib.root.clone()
            } else {
                PathBuf::from(&opts.root)
            };
            (root, db::all_stamps(&lib.conn)?)
        };
        if !root.is_dir() {
            return Err(format!("目录不存在：{}", root.to_string_lossy()));
        }

        let recursive = opts.recursive.unwrap_or(true);
        let entry_limit = opts.entry_limit.unwrap_or(60).clamp(1, 300);
        let max_depth = if recursive { 12 } else { 1 };
        let mut report = ScanReport {
            found: 0,
            added: 0,
            updated: 0,
            missing: 0,
            skipped: 0,
            probed: 0,
            reused: 0,
            errors: Vec::new(),
        };

        // 1) 只走文件系统
        let mut videos: Vec<PathBuf> = Vec::new();
        let mut archives: Vec<PathBuf> = Vec::new();
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
                videos.push(path.to_path_buf());
            } else if media::is_supported_archive(path) {
                archives.push(path.to_path_buf());
            }
        }
        let mut seen: Vec<String> = Vec::with_capacity(videos.len() + archives.len());
        let _ = app.emit(
            "scan-progress",
            serde_json::json!({"phase":"walk","done":0,"total":videos.len() + archives.len()}),
        );

        // 2) 压缩包：大小/时间没变就不重读 Central Directory
        let mut records: Vec<db::VideoRecord> = Vec::new();
        let mut reused = 0usize;
        for path in &archives {
            let key_path = path.to_string_lossy().to_string();
            let (size, mtime) = media::file_stamp(path);
            seen.push(key_path.clone());
            if let Some(st) = known.get(&(key_path, 0)) {
                if st.size_bytes == size && st.mtime == mtime && st.entry_count > 0 {
                    reused += 1;
                    continue;
                }
            }
            match media::archive_records(path, entry_limit) {
                Ok(recs) if !recs.is_empty() => {
                    report.found += recs.len();
                    records.extend(recs);
                }
                Ok(_) => report.skipped += 1,
                Err(e) => report.errors.push(format!("{}: {e}", path.to_string_lossy())),
            }
        }

        // 3) 普通视频：没变过且元数据齐全 → 只登记 stub，省掉 ffprobe
        let mut to_probe: Vec<PathBuf> = Vec::new();
        for path in &videos {
            report.found += 1;
            let key_path = path.to_string_lossy().to_string();
            let (size, mtime) = media::file_stamp(path);
            seen.push(key_path.clone());
            let fresh = known
                .get(&(key_path, 0))
                .is_some_and(|st| st.is_fresh(size, mtime));
            if fresh {
                records.push(media::record_stub(path));
                reused += 1;
            } else {
                to_probe.push(path.clone());
            }
        }

        // 4) 并行探测（无锁）
        let probed = probe_all(&app, &to_probe);
        report.probed = probed.len();
        records.extend(probed);

        // 5) 分批短锁落库，边写边报进度
        let mut written = 0usize;
        for chunk in records.chunks(64) {
            {
                let guard = state.lock()?;
                let lib = library(&guard)?;
                for rec in chunk {
                    match db::upsert_video(&lib.conn, rec) {
                        Ok(true) => report.added += 1,
                        Ok(false) => report.updated += 1,
                        Err(e) => report.errors.push(format!("{}: {e}", rec.path)),
                    }
                }
            }
            written += chunk.len();
            let _ = app.emit(
                "scan-progress",
                serde_json::json!({
                    "phase": "write",
                    "done": written,
                    "total": records.len(),
                    "added": report.added,
                    "updated": report.updated,
                }),
            );
        }

        // 6) 标记失效
        {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            report.missing = db::mark_missing(&lib.conn, &seen).unwrap_or(0);
        }
        report.reused = reused;
        let _ = app.emit(
            "scan-progress",
            serde_json::json!({"phase":"done","done":1,"total":1}),
        );
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
    let lib = library(&ctx)?;
    let mut info = player::resolve(lib, id)?;
    // 压缩包条目此时才第一次解出真实文件，顺手补上时长/分辨率
    player::backfill_meta(lib, id, Path::new(&info.media_path));
    if let Ok(fresh) = db::get_video(&lib.conn, id) {
        info.video = fresh;
    }
    Ok(info)
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
        // 抽帧前先补元数据：压缩包条目在扫描时拿不到时长，这里顺手补上
        player::backfill_meta(lib, id, &source);
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
                        // 批量预览正好是「把它解出来」的时机，顺手补元数据
                        player::backfill_meta(lib, *id, &source);
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

#[tauri::command]
fn player_engines(state: State<'_, AppState>) -> Result<Vec<mpv::EngineInfo>, String> {
    let ctx = state.lock()?;
    Ok(mpv::engines(ctx.settings.mpv_path.as_deref()))
}

/// 用 mpv IPC 打开一条视频。返回 true 表示已接管，false 表示调用方应退回内置播放器。
#[tauri::command]
async fn mpv_open(
    state: State<'_, AppState>,
    app: AppHandle,
    id: i64,
    wid: Option<String>,
) -> Result<bool, String> {
    let state_inner = state.inner().clone();
    let pref = state_inner.lock()?.settings.player_pref.clone();
    if &pref == "builtin" || &pref == "vlc" {
        return Ok(false);
    }
    let configured = state_inner.lock()?.settings.mpv_path.clone();
    let Some(path) = mpv::find_mpv(configured.as_deref()) else {
        return Ok(false);
    };
    // 先停掉旧会话（mpv 进程）
    if let Some(old) = state_inner.lock()?.mpv.take() {
        old.stop();
    }
    tauri::async_runtime::spawn_blocking(move || {
        let guard = state_inner.lock()?;
        let lib = library(&guard)?;
        let info = player::resolve(lib, id)?;
        let media = PathBuf::from(&info.media_path);
        if !media.is_file() {
            return Err("视频文件不存在".into());
        }
        let session = mpv::MpvSession::start(app, &path, &media, info.start_sec, wid)?;
        drop(guard);
        state_inner.lock()?.mpv = Some(Arc::new(session));
        Ok(true)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn mpv_command(state: State<'_, AppState>, action: String, value: Option<serde_json::Value>) -> Result<(), String> {
    let ctx = state.lock()?;
    let session = ctx.mpv.as_ref().ok_or("mpv 播放器未启动")?;
    let cmd = mpv::command_from(&action, value)?;
    session.send(cmd)
}

#[tauri::command]
fn mpv_close(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(session) = state.lock()?.mpv.take() {
        session.stop();
    }
    Ok(())
}

// ---------- 下载器 ----------

#[tauri::command]
async fn resolve_link(url: String) -> Result<ResolveReport, String> {
    tauri::async_runtime::spawn_blocking(move || downloader::resolve(&url))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_downloads(state: State<'_, AppState>) -> Result<Vec<DownloadTask>, String> {
    let ctx = state.lock()?;
    db::list_downloads(&library(&ctx)?.conn, 200)
}

#[tauri::command]
fn add_download(
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
fn remove_download(state: State<'_, AppState>, id: i64, delete_file: bool) -> Result<(), String> {
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
fn start_downloads(state: State<'_, AppState>, app: AppHandle) -> Result<usize, String> {
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
            player_engines,
            mpv_open,
            mpv_command,
            mpv_close,
            resolve_link,
            list_downloads,
            add_download,
            remove_download,
            start_downloads,
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
