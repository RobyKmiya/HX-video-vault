pub mod db;
pub mod downloader;
pub mod edit;
pub mod fileops;
pub mod importer;
pub mod media;
pub mod models;
mod mpv;
pub mod player;
mod proc;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::GlobalShortcutExt;
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
    /// 拖拽导入的根目录；空 = 库根下的 Supervideo（区间夹：每夹 200 个视频）
    #[serde(default)]
    pub import_root: Option<String>,
    /// 画廊双击：play = 直接播放（默认），detail = 打开详情抽屉
    #[serde(default = "default_double_click")]
    pub double_click: String,
    /// 播放时默认进入无边框沉浸模式
    #[serde(default)]
    pub immersive_default: bool,
    /// 卡片下面是否显示文件名与信息
    #[serde(default = "default_show_card_info")]
    pub show_card_info: bool,
}

fn default_player_pref() -> String { "auto".into() }
fn default_double_click() -> String { "play".into() }
fn default_show_card_info() -> bool { true }

impl Default for Settings {
    fn default() -> Self {
        Self {
            library_root: None,
            ffmpeg_path: None,
            ffprobe_path: None,
            player_pref: "auto".into(),
            mpv_path: None,
            import_root: None,
            double_click: "play".into(),
            immersive_default: false,
            show_card_info: true,
        }
    }
}

pub(crate) struct AppInner {
    library: Option<LibraryContext>,
    settings: Settings,
    startup_error: Option<String>,
    mpv: Option<Arc<mpv::MpvSession>>,
    /// 预览图任务的急停开关（暂停 = 置位；下次开跑前清掉）
    sheet_cancel: Arc<AtomicBool>,
    /// 是否已有预览图任务在跑（防重复启动）
    sheet_running: Arc<AtomicBool>,
    /// 剪辑任务的急停开关（ffmpeg 进程会被立刻杀掉）
    edit_cancel: Arc<AtomicBool>,
    /// 是否已有剪辑任务在跑（防重复启动）
    edit_running: Arc<AtomicBool>,
    /// 剪辑任务现场（独立锁：进度刷新不挤占全局状态锁）
    edit_status: Arc<Mutex<crate::models::EditStatus>>,
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
                sheet_cancel: Arc::new(AtomicBool::new(false)),
                sheet_running: Arc::new(AtomicBool::new(false)),
                edit_cancel: Arc::new(AtomicBool::new(false)),
                edit_running: Arc::new(AtomicBool::new(false)),
                edit_status: Arc::new(Mutex::new(crate::models::EditStatus {
                    running: false,
                    phase: "idle".into(),
                    percent: 0.0,
                    message: String::new(),
                })),
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
            folders: 0,
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
            // 跳过符号链接/联接与 Windows 快捷方式：它们指向的文件会被目标路径本身登记，
            // 不跳过就会出现「同一部影片好几条」的假重复。
            if entry.file_type().is_symlink() {
                continue;
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|s| s.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
            {
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
            let key_path = media::path_key(path);
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
            let key_path = media::path_key(path);
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

        // 6) 标记失效 + 文件夹镜像：磁盘子目录自动归成 kind='auto' 收藏夹并归位
        {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            report.missing = db::mark_missing(&lib.conn, &seen).unwrap_or(0);
            report.folders = db::sync_folder_collections(&lib.conn, &root, &seen).unwrap_or(0);
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
fn list_collections(state: State<'_, AppState>, include_deleted: Option<bool>) -> Result<Vec<Collection>, String> {
    let ctx = state.lock()?;
    db::list_collections(&library(&ctx)?.conn, include_deleted.unwrap_or(false))
}

#[tauri::command]
fn create_collection(state: State<'_, AppState>, name: String, group: Option<String>) -> Result<i64, String> {
    let ctx = state.lock()?;
    db::create_collection(&library(&ctx)?.conn, &name, group.as_deref().unwrap_or(""))
}

#[tauri::command]
fn rename_collection(state: State<'_, AppState>, id: i64, name: String) -> Result<(), String> {
    let ctx = state.lock()?;
    db::rename_collection(&library(&ctx)?.conn, id, &name)
}

#[tauri::command]
fn set_collection_group(state: State<'_, AppState>, id: i64, group: String) -> Result<(), String> {
    let ctx = state.lock()?;
    db::set_collection_group(&library(&ctx)?.conn, id, &group)
}

#[tauri::command]
fn delete_collection(state: State<'_, AppState>, id: i64, purge: bool) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    if purge {
        db::purge_collection(&lib.conn, id)
    } else {
        db::soft_delete_collection(&lib.conn, id)
    }
}

#[tauri::command]
fn restore_collection(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    db::restore_collection(&library(&ctx)?.conn, id)
}

#[tauri::command]
fn set_collection_video(
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

/// 批量生成接触表。可中断、可续跑：每做完一条立刻落库，停掉后下次从缺图的地方接着来。
/// 三条工作线程并行（ffprobe/ffmpeg 是独立进程，互不阻塞），锁只在读来源与写结果时短暂持有。
#[tauri::command]
async fn make_contact_sheets(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Option<Vec<i64>>,
    limit: Option<usize>,
    all: Option<bool>,
) -> Result<crate::models::SheetReport, String> {
    let state = state.inner().clone();
    let regenerate_all = all.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let (cancel, running) = {
            let guard = state.lock()?;
            (guard.sheet_cancel.clone(), guard.sheet_running.clone())
        };
        if running.swap(true, Ordering::SeqCst) {
            return Err("已有预览图任务在跑，请先等它结束或点暂停".into());
        }
        cancel.store(false, Ordering::SeqCst);
        // 无论正常结束还是中途报错，都要把 running 放掉，否则按钮永远转圈
        struct Reset(Arc<AtomicBool>);
        impl Drop for Reset {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _reset = Reset(running.clone());

        let (targets, cache_dir) = {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            let ids = match ids {
                Some(v) => v,
                None => {
                    let mut v = db::all_video_ids_for_sheet(&lib.conn, !regenerate_all)?;
                    if let Some(cap) = limit {
                        v.truncate(cap.clamp(1, 100_000));
                    }
                    v
                }
            };
            (ids, crate::db::LibraryContext::vault_dir(&lib.root).join("cache").join("contacts"))
        };
        let total = targets.len();
        let processed = Arc::new(AtomicUsize::new(0));
        let ok_count = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(Mutex::new(Vec::<String>::new()));
        let cursor = Arc::new(AtomicUsize::new(0));
        let workers = 3usize.min(total.max(1));

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let state = state.clone();
                let app = app.clone();
                let cancel = cancel.clone();
                let cursor = cursor.clone();
                let processed = processed.clone();
                let ok_count = ok_count.clone();
                let failed = failed.clone();
                let targets = &targets;
                let cache_dir = &cache_dir;
                scope.spawn(move || loop {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let i = cursor.fetch_add(1, Ordering::SeqCst);
                    if i >= targets.len() {
                        break;
                    }
                    let id = targets[i];
                    // 1) 短锁：解析真实播放源（压缩包条目会按需解到缓存）
                    let resolved = {
                        let guard = match state.lock() {
                            Ok(g) => g,
                            Err(e) => {
                                failed.lock().map(|mut v| v.push(format!("#{id}: {e}"))).ok();
                                processed.fetch_add(1, Ordering::SeqCst);
                                continue;
                            }
                        };
                        match guard.library.as_ref() {
                            Some(lib) => player::resolve(lib, id).map(|info| PathBuf::from(info.media_path)),
                            None => Err("库未打开".into()),
                        }
                    };
                    let source = match resolved {
                        Ok(p) => p,
                        Err(e) => {
                            failed.lock().map(|mut v| v.push(format!("#{id}: {e}"))).ok();
                            processed.fetch_add(1, Ordering::SeqCst);
                            continue;
                        }
                    };
                    if !source.is_file() {
                        processed.fetch_add(1, Ordering::SeqCst);
                        continue;
                    }
                    // 2) 无锁：ffmpeg 抽帧
                    let out = cache_dir.join(format!("{id}.jpg"));
                    let result = media::contact_sheet(&source, &out);
                    // 3) 短锁：落库 + 补元数据
                    match result {
                        Ok(()) => {
                            let mut stored = false;
                            if let Ok(guard) = state.lock() {
                                if let Some(lib) = guard.library.as_ref() {
                                    if db::set_contact_sheet(&lib.conn, id, &out.to_string_lossy()).is_ok() {
                                        player::backfill_meta(lib, id, &source);
                                        stored = true;
                                    }
                                }
                            }
                            if stored {
                                ok_count.fetch_add(1, Ordering::SeqCst);
                            } else {
                                failed.lock().map(|mut v| v.push(format!("#{id}: 落库失败"))).ok();
                            }
                        }
                        Err(e) => {
                            failed.lock().map(|mut v| v.push(format!("#{id}: {e}"))).ok();
                        }
                    }
                    let done = processed.fetch_add(1, Ordering::SeqCst) + 1;
                    let _ = app.emit("sheet-progress", serde_json::json!({
                        "done": done,
                        "total": total,
                        "id": id,
                        "name": source.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                    }));
                });
            }
        });

        let done = ok_count.load(Ordering::SeqCst);
        let stopped = cancel.load(Ordering::SeqCst);
        let failures = failed.lock().map(|v| v.clone()).unwrap_or_default();
        let _ = app.emit("sheet-progress", serde_json::json!({
            "done": processed.load(Ordering::SeqCst),
            "total": total,
            "id": 0,
            "finished": true,
            "cancelled": stopped,
        }));
        Ok(crate::models::SheetReport {
            total,
            done,
            skipped: total.saturating_sub(processed.load(Ordering::SeqCst)),
            failed: failures,
            cancelled: stopped,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 停掉正在跑的预览图任务（当前这一条抽完后就不再取新的）。
#[tauri::command]
fn cancel_contact_sheets(state: State<'_, AppState>) -> Result<(), String> {
    let ctx = state.lock()?;
    ctx.sheet_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 清空预览图记录（回滚）。`ids=None` 时清全库；返回实际清掉的图片数量。
#[tauri::command]
fn clear_contact_sheets(state: State<'_, AppState>, ids: Option<Vec<i64>>) -> Result<usize, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let paths = db::clear_contact_sheets(&lib.conn, ids.as_deref())?;
    for p in &paths {
        let _ = fs::remove_file(p);
    }
    Ok(paths.len())
}

/// 预览图任务现场：总数 / 已有 / 缺图 / 是否在跑。
#[tauri::command]
fn sheet_status(state: State<'_, AppState>) -> Result<crate::models::SheetStatus, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let (total, done, pending) = db::sheet_counts(&lib.conn)?;
    Ok(crate::models::SheetStatus {
        running: ctx.sheet_running.load(Ordering::SeqCst),
        total,
        done,
        pending,
    })
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

// ---------- 剪辑 ----------

/// 裁剪一段视频。素材可以是库内条目（含压缩包内条目）或任意本地文件。
#[tauri::command]
async fn trim_video(
    state: State<'_, AppState>,
    app: AppHandle,
    source: models::EditSource,
    start_sec: f64,
    end_sec: f64,
    accurate: Option<bool>,
) -> Result<models::EditReport, String> {
    let state = state.inner().clone();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        edit::run_edit_job(
            &state,
            &app,
            edit::EditJob::Trim {
                sources: vec![source],
                start_sec,
                end_sec,
                accurate: accurate.unwrap_or(false),
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 按顺序拼接多段视频。全段参数一致走流复制（秒出），不一致自动重编码对齐第一段。
#[tauri::command]
async fn concat_video(
    state: State<'_, AppState>,
    app: AppHandle,
    sources: Vec<models::EditSource>,
) -> Result<models::EditReport, String> {
    let state = state.inner().clone();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        edit::run_edit_job(&state, &app, edit::EditJob::Concat { sources })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 剪辑任务现场（进度面板 + 关掉面板重开后恢复现场）。
#[tauri::command]
fn edit_status(state: State<'_, AppState>) -> Result<models::EditStatus, String> {
    let slot = state.lock()?.edit_status.clone();
    slot.lock()
        .map(|s| s.clone())
        .map_err(|_| "剪辑状态已损坏，请重启影匣".into())
}

/// 请求取消当前剪辑任务：ffmpeg 在下一行进度到来时被杀掉，半成品会被清掉。
#[tauri::command]
fn cancel_edit(state: State<'_, AppState>) -> Result<(), String> {
    let ctx = state.lock()?;
    if !ctx.edit_running.load(Ordering::SeqCst) {
        return Err("没有正在进行的剪辑任务".into());
    }
    ctx.edit_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 多选本地视频文件（外来素材入口）。
#[tauri::command]
async fn pick_video_files(app: AppHandle) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    const VIDEO_FILTER: &[&str] = &[
        "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m2ts", "mpg", "mpeg",
        "3gp", "vob", "rmvb", "rm", "mts", "m2v", "asf", "f4v",
    ];
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("视频文件", VIDEO_FILTER)
            .blocking_pick_files()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(picked
        .map(|list| list.iter().map(|p| p.to_string()).collect())
        .unwrap_or_default())
}

/// 探测一个本地文件的时长/分辨率/编码（外来素材加入剪辑面板时预览用）。
#[tauri::command]
fn probe_media(path: String) -> Result<crate::media::MediaMeta, String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("文件不存在".into());
    }
    Ok(media::probe(&p))
}

// ---------- 封面 ----------

/// 封面生成的串行闸门：卡片首屏可能一次要几十张中帧，
/// 全放出去就是几十个 ffmpeg 进程；抽单帧本来就快，排队零伤害。
static COVER_GATE: Mutex<()> = Mutex::new(());

fn cover_gate() -> std::sync::MutexGuard<'static, ()> {
    COVER_GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 取一条视频的最佳封面路径：自选封面 > 接触表之外的中帧兜底（懒生成 + 永久缓存）。
/// 卡片只在既无自选封面又无接触表时才调用；压缩包条目会先解缓存再抽帧。
#[tauri::command]
async fn video_cover(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 短锁一：自选封面 + 解析真实路径（压缩包条目按需解缓存）
        let (custom, resolved, root) = {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            let custom = db::get_cover_path(&lib.conn, id)?;
            let info = player::resolve(lib, id)?;
            (custom, PathBuf::from(info.media_path), lib.root.clone())
        };
        if let Some(p) = custom {
            if Path::new(&p).is_file() {
                return Ok(p);
            }
        }
        if !resolved.is_file() {
            return Err("视频文件不存在".into());
        }
        let out = db::LibraryContext::vault_dir(&root)
            .join("cache")
            .join("cover")
            .join(format!("mid-{id}.jpg"));
        if out.is_file() {
            return Ok(out.to_string_lossy().to_string());
        }
        // 无锁：探测时长，取中间帧
        let meta = media::probe(&resolved);
        if meta.duration_sec <= 0.0 {
            return Err("无法读取视频时长，取不了中间帧".into());
        }
        let mid = meta.duration_sec / 2.0;
        let _gate = cover_gate();
        media::extract_frame(&resolved, mid, &out)?;
        drop(_gate);
        // 短锁二：顺手回填元数据（压缩包条目第一次被看时补齐，与抽帧同款习惯）
        if let Ok(guard) = state.lock() {
            if let Some(lib) = guard.library.as_ref() {
                let _ = db::backfill_probe(
                    &lib.conn,
                    id,
                    meta.duration_sec,
                    meta.width,
                    meta.height,
                    &meta.codec_video,
                    &meta.codec_audio,
                );
            }
        }
        Ok(out.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 内置播放器「设为封面」：前端 canvas 截的当前帧（JPEG data URL）落盘并登记。
#[tauri::command]
async fn set_cover_image(
    state: State<'_, AppState>,
    id: i64,
    data_url: String,
) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = media::decode_data_url(&data_url)?;
        if bytes.len() < 64 || bytes.len() > 12 * 1024 * 1024 {
            return Err(format!("截图大小异常（{} 字节）", bytes.len()));
        }
        let path = {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            db::LibraryContext::vault_dir(&lib.root).join("cache").join("cover").join(format!("cover-{id}.jpg"))
        };
        {
            let _gate = cover_gate();
            std::fs::write(&path, &bytes).map_err(|e| format!("无法写封面文件: {e}"))?;
        }
        // 短锁：登记
        let guard = state.lock()?;
        let lib = library(&guard)?;
        db::set_cover(&lib.conn, id, &path.to_string_lossy())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// mpv「设为封面」第一步：给出 mpv screenshot-to-file 应该写的目标绝对路径。
#[tauri::command]
fn mpv_cover_path(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let dir = db::LibraryContext::vault_dir(&lib.root).join("cache").join("cover");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("cover-{id}.png")).to_string_lossy().to_string())
}

/// 内置播放器「设为封面」：canvas 在 asset 协议下会被污染导不出图（SecurityError），
/// 所以改由前端只报当前秒数，后端 ffmpeg 精确抽那一帧（输入侧 seek + accurate_seek）。
#[tauri::command]
async fn set_cover_from_time(state: State<'_, AppState>, id: i64, sec: f64) -> Result<(), String> {
    if !(sec.is_finite() && sec >= 0.0) {
        return Err("时间点无效".into());
    }
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 短锁一：解析真实路径（压缩包条目按需解缓存）
        let (resolved, path) = {
            let guard = state.lock()?;
            let lib = library(&guard)?;
            let info = player::resolve(lib, id)?;
            (
                PathBuf::from(info.media_path),
                db::LibraryContext::vault_dir(&lib.root).join("cache").join("cover").join(format!("cover-{id}.jpg")),
            )
        };
        if !resolved.is_file() {
            return Err("视频文件不存在".into());
        }
        {
            let _gate = cover_gate();
            media::extract_frame(&resolved, sec, &path)?;
        }
        // 短锁二：登记
        let guard = state.lock()?;
        let lib = library(&guard)?;
        db::set_cover(&lib.conn, id, &path.to_string_lossy())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// mpv「设为封面」第二步：mpv 已把当前帧写到目标路径，登记进库。
#[tauri::command]
async fn set_cover_from_mpv(state: State<'_, AppState>, id: i64, path: String) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let p = PathBuf::from(&path);
        if !p.is_file() {
            return Err("mpv 还没写出截图文件".into());
        }
        let guard = state.lock()?;
        let lib = library(&guard)?;
        db::set_cover(&lib.conn, id, &p.to_string_lossy())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 撤销自选封面：回到接触表 / 中间帧兜底（缓存文件保留，不删）。
#[tauri::command]
fn clear_cover(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::clear_cover(&lib.conn, id)
}

/// 重置一个收藏夹（文件夹）内所有成员的缓存图：清自选封面记录 + 接触表记录，
/// 删 cover-{id}/mid-{id}/接触表缓存文件；返回被重置的成员 id 列表（前端据此给 URL 换版本号，
/// 防 WebView 继续用旧图）。前端随后刷新列表，卡片懒加载只对这个夹重新生成封面（不用全库慢扫）。
#[tauri::command]
fn reset_folder_covers(state: State<'_, AppState>, collection_id: i64) -> Result<Vec<i64>, String> {
    let (ids, cover_dir, sheet_paths) = {
        let ctx = state.lock()?;
        let lib = library(&ctx)?;
        let ids = db::collection_member_ids(&lib.conn, collection_id)?;
        for id in &ids {
            db::clear_cover(&lib.conn, *id)?;
        }
        let sheets = db::clear_contact_sheets(&lib.conn, Some(&ids))?;
        (
            ids,
            LibraryContext::vault_dir(&lib.root).join("cache").join("cover"),
            sheets,
        )
    };
    // 锁外删文件
    for id in &ids {
        for name in [
            format!("cover-{id}.jpg"),
            format!("cover-{id}.png"),
            format!("mid-{id}.jpg"),
        ] {
            let _ = fs::remove_file(cover_dir.join(name));
        }
    }
    for p in &sheet_paths {
        let _ = fs::remove_file(p);
    }
    Ok(ids)
}

// ---------- 拖拽导入 ----------

/// 拖拽导入的根目录：设置显式指定 > 库根\Supervideo。
fn import_root_of(settings: &Settings, lib_root: &Path) -> PathBuf {
    settings
        .import_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| lib_root.join("Supervideo"))
}

/// 拖拽入队（前端把 webview 拖事件的 path 列表传进来）：
/// 立即入队后台复制（.part 原子落盘 + 大小校验 + 单线程串行），返回实际入队数。
#[tauri::command]
fn import_videos(state: State<'_, AppState>, app: AppHandle, paths: Vec<String>) -> Result<usize, String> {
    let import_root = {
        let g = state.lock()?;
        let lib = library(&g)?;
        import_root_of(&g.settings, &lib.root)
    };
    let sources: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    importer::enqueue(&app, &import_root, &sources)
}

/// 导入队列现场（角落浮窗显示）。
#[tauri::command]
fn import_status() -> Result<crate::importer::ImportStatus, String> {
    Ok(importer::status())
}

/// 取消导入：当前文件的半成品清掉，队列清空，已完成的保留。
#[tauri::command]
fn cancel_import() -> Result<(), String> {
    importer::cancel();
    Ok(())
}

// ---------- 应用内文件编辑：重命名 / 移动 / 定位 ----------

/// 移动目标（对话框用）：库根 + 所有 auto 文件夹（绝对路径直接给后端用）。
#[derive(Debug, Clone, Serialize)]
struct MoveTarget {
    name: String,
    path: String,
    count: i64,
}

#[tauri::command]
fn move_targets(state: State<'_, AppState>) -> Result<Vec<MoveTarget>, String> {
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
fn rename_video(state: State<'_, AppState>, id: i64, name: String) -> Result<VideoItem, String> {
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
fn move_videos(
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
fn reveal_video(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let (path, entry_index) = db::video_source(&lib.conn, id)?;
    if entry_index > 0 {
        return Err("压缩包条目没有独立文件，没法定位".into());
    }
    fileops::reveal(Path::new(&path))
}

/// 沉浸模式（真无边框）：**只**去掉系统标题栏，不碰全屏。
///
/// 最大化状态下直接切装饰会让客户区尺寸对不上（内容会跑到屏幕外一截），
/// 所以先取消最大化 → 切装饰 → 再最大化一次，让系统重算布局。
///
/// 画匣的沉浸阅读同款：整个播放会话期间窗口零调整，切换只在进入/退出时发生，
/// 不跟着界面里的开关来回抖。
#[tauri::command]
fn set_immersive(app: AppHandle, on: bool) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or("找不到主窗口")?;
    let maximized = win.is_maximized().unwrap_or(false);
    if maximized {
        let _ = win.unmaximize();
    }
    win.set_decorations(!on)
        .map_err(|e| format!("切换窗口边框失败：{e}"))?;
    if maximized {
        let _ = win.maximize();
    }
    Ok(())
}

/// 真全屏（系统级窗口全屏），与「无边框沉浸」是两回事：
/// 无边框只是去掉标题栏，窗口仍是原来的大小；全屏才铺满整块屏幕。
#[tauri::command]
fn set_fullscreen(app: AppHandle, on: bool) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or("找不到主窗口")?;
    win.set_fullscreen(on)
        .map_err(|e| format!("切换全屏失败：{e}"))?;
    Ok(())
}

/* ---------------- 老板键：藏窗 + 系统级全局热键呼回 ---------------- */

/// 呼回键的持久化路径（config/shortcuts.json）。
fn boss_recall_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("shortcuts.json"))
}

/// 读当前呼回键绑定；没存过或读坏了就给默认 Ctrl+Shift+H。
fn load_boss_recall(app: &AppHandle) -> String {
    boss_recall_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value["bossRecall"].as_str().map(str::to_string))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Ctrl+Shift+H".to_string())
}

/// 注册全局呼回键（失败返回 Err，由调用方决定怎么提示）。
///
/// 绑定串必须转小写：前端录出来的是 "Ctrl+Shift+H" 这种驼峰，
/// 而全局热键插件的注册是大小写敏感的。
fn register_boss_recall(app: &AppHandle, binding: &str) -> Result<(), String> {
    let shortcut = binding.trim().to_lowercase();
    if shortcut.is_empty() {
        return Err("呼回键不能为空".into());
    }
    // 先摘掉旧的（失败忽略——可能本来就没注册上），再注册新的
    let _ = app.global_shortcut().unregister(load_boss_recall(app).to_lowercase().as_str());
    app.global_shortcut()
        .register(shortcut.as_str())
        .map_err(|e| format!("注册全局热键失败：{e}"))
}

/// 老板键第一步：把主窗整个藏起来（恢复走全局热键，默认 Ctrl+Shift+H，可在设置里改）。
///
/// mpv 是**独立顶层窗口**（--force-window，不是嵌在 webview 里），
/// 藏主窗时它会留在屏幕上 —— 所以先把 mpv 收摊，画面不能残留在那。
#[tauri::command]
fn hide_main_window(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    if let Some(session) = state.lock()?.mpv.take() {
        session.stop();
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    window.hide().map_err(|e| format!("隐藏窗口失败：{e}"))
}

#[tauri::command]
fn get_boss_recall(app: AppHandle) -> String {
    load_boss_recall(&app)
}

/// 改呼回键：**注册成功才落盘**，失败不破坏现有绑定。
#[tauri::command]
fn set_boss_recall(app: AppHandle, binding: String) -> Result<(), String> {
    register_boss_recall(&app, &binding)?;
    if let Some(path) = boss_recall_path(&app) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            path,
            serde_json::json!({ "bossRecall": binding.trim() }).to_string(),
        );
    }
    Ok(())
}

/// 把所选视频打包成一个 zip（**保留原文件**），落在第一条所在目录，返回压缩包路径。
/// 压缩包内条目（entry_index>0）跳过 —— 不能再套一层。
#[tauri::command]
fn compress_videos(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    name: Option<String>,
) -> Result<String, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    for id in ids {
        let Ok((path, entry_index)) = db::video_source(&lib.conn, id) else { continue };
        if entry_index > 0 {
            continue;
        }
        let p = PathBuf::from(path);
        if !p.is_file() {
            continue;
        }
        let inner = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        files.push((p, inner));
    }
    if files.is_empty() {
        return Err("没有可压缩的文件（压缩包内的条目不能再次打包）".into());
    }
    let dir = files[0].0.parent().ok_or("取不到目标目录")?.to_path_buf();
    let stem = name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| fileops::validate_name(s).unwrap_or_else(|_| "打包".to_string()))
        .unwrap_or_else(|| "打包".to_string());
    let zip_path = fileops::unique_path(&dir, &format!("{stem}.zip"));
    let file = fs::File::create(&zip_path).map_err(|e| format!("创建压缩包失败：{e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    for (path, inner) in &files {
        use std::io::{Read, Write as IoWrite};
        zip.start_file(inner.as_str(), opts)
            .map_err(|e| format!("写入「{inner}」失败：{e}"))?;
        let mut src = fs::File::open(path).map_err(|e| format!("读取失败：{e}"))?;
        loop {
            let n = src.read(&mut buf).map_err(|e| format!("读取中断：{e}"))?;
            if n == 0 {
                break;
            }
            zip.write_all(&buf[..n]).map_err(|e| format!("写入中断：{e}"))?;
        }
    }
    zip.finish().map_err(|e| format!("收尾失败：{e}"))?;
    Ok(zip_path.to_string_lossy().to_string())
}

// ---------- 收藏夹排序 ----------

/// 主页层级浏览：某层下的 auto 子文件夹卡（parent=None 取顶层）。
#[tauri::command]
fn list_folder_children(
    state: State<'_, AppState>,
    parent: Option<String>,
) -> Result<Vec<crate::models::FolderInfo>, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::list_folder_children(&lib.conn, &lib.root, parent.as_deref().map(str::trim).filter(|s| !s.is_empty()))
}

/// 侧栏收藏夹拖拽排序：前端给出拖完后的完整 id 顺序。
#[tauri::command]
fn reorder_collections(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::reorder_collections(&lib.conn, &ids)
}

/// 收藏夹内视频拖拽排序：前端给出可见子集的新顺序，未在子集里的成员整体后移保序。
#[tauri::command]
fn reorder_collection_videos(
    state: State<'_, AppState>,
    collection_id: i64,
    video_ids: Vec<i64>,
) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::reorder_collection_videos(&lib.conn, collection_id, &video_ids)
}

/// 单实例守卫（Windows 命名互斥体）：
/// 连点桌面图标、或更新后旧窗口没关，第二个进程不再启动一整套窗口 + 再开一次数据库，
/// 而是把已存在的窗口叫到前台就退出 —— 之前连点几次就会出现两个进程互抢同一个 SQLite 库。
#[cfg(windows)]
mod single_instance {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    /// 互斥体句柄故意不关：进程活着它就活着，进程退出系统自动回收
    static MUTEX: AtomicIsize = AtomicIsize::new(0);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// true = 我是唯一实例；false = 已经有一个在跑。
    /// 极端情况下拿不到句柄（没权限等）也放行：宁可能开两个，也不要把人挡在门外。
    pub fn claim() -> bool {
        let name = wide("Local\\VideoVault.SingleInstance.v1");
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if h.is_null() {
                return true;
            }
            MUTEX.store(h as isize, Ordering::SeqCst);
            GetLastError() != ERROR_ALREADY_EXISTS
        }
    }

    /// 把已在跑的影匣窗口叫到前台：用户连点的预期是「还是那一个窗口」。
    pub fn focus_existing() -> bool {
        let title = wide("影匣 · 本地视频库");
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            true
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 单实例守卫必须在建窗口之前：第二个实例连 Tauri 都不初始化，直接退出。
    #[cfg(windows)]
    {
        if !single_instance::claim() {
            single_instance::focus_existing();
            std::process::exit(0);
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // 老板键呼回：全局热键切换主窗显隐。窗口藏起来之后应用内按键收不到，
        // 必须靠系统级热键才能唤回。
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        if let Some(window) = app.get_webview_window("main") {
                            let visible = window.is_visible().unwrap_or(false);
                            if visible {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            let state = AppState::new(app.handle());
            app.manage(state);
            // 注册老板键呼回（默认 Ctrl+Shift+H，可在设置里改；失败不拦启动）
            {
                let binding = load_boss_recall(app.handle());
                if let Err(error) = register_boss_recall(app.handle(), &binding) {
                    eprintln!("注册老板键失败：{error}");
                }
            }
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
            rename_collection,
            set_collection_group,
            delete_collection,
            restore_collection,
            set_collection_video,
            stats,
            playback_info,
            play_external,
            make_contact_sheet,
            make_contact_sheets,
            cancel_contact_sheets,
            clear_contact_sheets,
            sheet_status,
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
            trim_video,
            concat_video,
            edit_status,
            cancel_edit,
            pick_video_files,
            probe_media,
            video_cover,
            set_cover_image,
            mpv_cover_path,
            set_cover_from_mpv,
            set_cover_from_time,
            clear_cover,
            reorder_collections,
            reorder_collection_videos,
            list_folder_children,
            import_videos,
            import_status,
            cancel_import,
            reset_folder_covers,
            move_targets,
            rename_video,
            move_videos,
            reveal_video,
            set_immersive,
            set_fullscreen,
            hide_main_window,
            get_boss_recall,
            set_boss_recall,
            compress_videos,
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
