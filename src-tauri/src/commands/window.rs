//! `window` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

/// 沉浸模式（真无边框）：**只**去掉系统标题栏，不碰全屏。
///
/// 最大化状态下直接切装饰会让客户区尺寸对不上（内容会跑到屏幕外一截），
/// 所以先取消最大化 → 切装饰 → 再最大化一次，让系统重算布局。
///
/// 画匣的沉浸阅读同款：整个播放会话期间窗口零调整，切换只在进入/退出时发生，
/// 不跟着界面里的开关来回抖。
#[tauri::command]
pub(crate) fn set_immersive(app: AppHandle, on: bool) -> Result<(), String> {
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
pub(crate) fn set_fullscreen(app: AppHandle, on: bool) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or("找不到主窗口")?;
    win.set_fullscreen(on)
        .map_err(|e| format!("切换全屏失败：{e}"))?;
    Ok(())
}

/* ---------------- 老板键：藏窗 + 系统级全局热键呼回 ---------------- */

/// 呼回键的持久化路径（config/shortcuts.json）。
pub(crate) fn boss_recall_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("shortcuts.json"))
}

/// 读当前呼回键绑定；没存过或读坏了就给默认 Ctrl+Shift+H。
pub(crate) fn load_boss_recall(app: &AppHandle) -> String {
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
pub(crate) fn register_boss_recall(app: &AppHandle, binding: &str) -> Result<(), String> {
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
pub(crate) fn hide_main_window(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    if let Some(session) = state.lock()?.mpv.take() {
        session.stop();
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    window.hide().map_err(|e| format!("隐藏窗口失败：{e}"))
}

#[tauri::command]
pub(crate) fn get_boss_recall(app: AppHandle) -> String {
    load_boss_recall(&app)
}

/// 改呼回键：**注册成功才落盘**，失败不破坏现有绑定。
#[tauri::command]
pub(crate) fn set_boss_recall(app: AppHandle, binding: String) -> Result<(), String> {
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
