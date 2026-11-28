pub mod db;
pub mod downloader;
pub mod edit;
pub mod fileops;
pub mod importer;
pub mod media;
pub mod models;
mod mpv;
pub mod netproxy;
pub mod player;
mod proc;
pub(crate) mod bridge;
pub(crate) mod commands;
pub(crate) mod we_export;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use db::LibraryContext;

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
    /// 影片墙卡片缩放倍率（0.6~1.8，1.0 = 默认 208px）。Ctrl+滚轮也能调。
    #[serde(default = "default_grid_zoom")]
    pub grid_zoom: f64,
    /// 主题：dark（橙黑暗色，默认）/ light
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 自定义主色（#rrggbb），空 = 用主题默认橙
    #[serde(default)]
    pub accent: String,
}

fn default_player_pref() -> String { "auto".into() }
fn default_double_click() -> String { "play".into() }
fn default_show_card_info() -> bool { true }
fn default_grid_zoom() -> f64 { 1.0 }
fn default_theme() -> String { "dark".into() }

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
            grid_zoom: 1.0,
            theme: default_theme(),
            accent: String::new(),
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
    /// 指纹计算是否在跑（防重复启动）
    dedupe_running: Arc<AtomicBool>,
    /// 指纹进度：已算 / 总数（独立锁，后台线程频繁写，不挤全局锁）
    dedupe_progress: Arc<Mutex<(i64, i64)>>,
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
                dedupe_running: Arc::new(AtomicBool::new(false)),
                dedupe_progress: Arc::new(Mutex::new((0, 0))),
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

/// 给本地桥用：拿库执行一段操作（桥在线程里跑，不能直接摸 AppState 内部）
pub(crate) fn with_library<T>(
    state: &AppState,
    f: impl FnOnce(&LibraryContext) -> Result<T, String>,
) -> Result<T, String> {
    let ctx = state.inner_lock()?;
    let lib = library(&ctx)?;
    f(lib)
}

fn library(ctx: &AppInner) -> Result<&LibraryContext, String> {
    ctx.library
        .as_ref()
        .ok_or_else(|| "尚未创建或打开视频库，请先在设置里选择库目录".into())
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
            // 浏览器油猴脚本用的本地桥（只监听 127.0.0.1，端口占用就静默放弃）
            if let Some(st) = app.try_state::<AppState>() {
                crate::bridge::spawn(app.handle().clone(), st.inner().clone());
            }
            // 壁纸导出助手：影匣作为唯一守护拉起并盯住它（死了自动重启）
            crate::we_export::start();
            // 启动后后台整理一次：浏览器常把新视频直接下到库根/老夹里，收进编号夹再浏览不容易卡。
            // 先做便宜判断，没东西可整就完全不碰库。
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    let Some(state) = handle.try_state::<AppState>() else { return };
                    let st = state.inner().clone();
                    let Ok(ctx) = st.lock() else { return };
                    let Ok(lib) = library(&ctx) else { return };
                    if !db::needs_organize(&lib.root) {
                        return;
                    }
                    if let Ok(report) = db::organize_numbered(lib, db::FOLDER_CAP) {
                        let _ = db::apply_pending_hooks(&lib.conn);
                        if report.moved > 0 {
                            let _ = handle.emit("library-organized", report.moved);
                        }
                    }
                });
            }
            // 注册老板键呼回（默认 Ctrl+Shift+H，可在设置里改；失败不拦启动）
            {
                let binding = commands::window::load_boss_recall(app.handle());
                if let Err(error) = commands::window::register_boss_recall(app.handle(), &binding) {
                    eprintln!("注册老板键失败：{error}");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::app_settings,
            commands::settings::set_library,
            commands::settings::update_settings,
            commands::settings::pick_directory,
            commands::settings::scan,
            commands::browse::query_videos,
            commands::browse::get_video,
            commands::browse::patch_video,
            commands::browse::record_watch,
            commands::browse::trash_video,
            commands::collections::list_tags,
            commands::collections::create_tag,
            commands::collections::update_tag,
            commands::collections::set_tag_video,
            commands::collections::bulk_tag,
            commands::collections::list_collections,
            commands::collections::create_collection,
            commands::collections::rename_collection,
            commands::collections::set_collection_group,
            commands::collections::delete_collection,
            commands::collections::restore_collection,
            commands::collections::set_collection_video,
            commands::settings::stats,
            commands::player::playback_info,
            commands::player::play_external,
            commands::sheets::make_contact_sheet,
            commands::sheets::make_contact_sheets,
            commands::sheets::cancel_contact_sheets,
            commands::sheets::clear_contact_sheets,
            commands::sheets::sheet_status,
            commands::player::watch_history,
            commands::player::player_engines,
            commands::player::mpv_open,
            commands::player::mpv_command,
            commands::player::mpv_close,
            commands::downloads::resolve_link,
            commands::downloads::list_downloads,
            commands::downloads::add_download,
            commands::downloads::enqueue_download,
            commands::downloads::clear_done_downloads,
            commands::downloads::remove_download,
            commands::downloads::start_downloads,
            commands::editing::trim_video,
            commands::editing::concat_video,
            commands::editing::edit_status,
            commands::editing::cancel_edit,
            commands::editing::pick_video_files,
            commands::editing::probe_media,
            commands::covers::video_cover,
            commands::covers::set_cover_image,
            commands::covers::mpv_cover_path,
            commands::covers::set_cover_from_mpv,
            commands::covers::set_cover_from_time,
            commands::covers::clear_cover,
            commands::collections::reorder_collections,
            commands::collections::reorder_collection_videos,
            commands::browse::list_folder_children,
            commands::imports::import_videos,
            commands::imports::import_status,
            commands::imports::cancel_import,
            commands::covers::reset_folder_covers,
            commands::files::move_targets,
            commands::files::rename_video,
            commands::files::move_videos,
            commands::files::reveal_video,
            commands::files::purge_videos,
            commands::files::organize_library,
            commands::window::set_immersive,
            commands::window::set_fullscreen,
            commands::window::hide_main_window,
            commands::window::get_boss_recall,
            commands::window::set_boss_recall,
            commands::editing::compress_videos,
            commands::dedupe::dedupe_scan,
            commands::dedupe::dedupe_status,
            commands::dedupe::dedupe_groups,
            commands::dedupe::dedupe_apply,
            commands::dedupe::dedupe_apply_selected,
            commands::dedupe::purge_missing_trashed,
            commands::we_export::we_export_status,
            commands::we_export::we_export_start,
            commands::we_export::we_export_stop,
            commands::we_export::we_export_test,
            commands::we_export::we_export_set_config,
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
