//! `imports` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

// ---------- 拖拽导入 ----------

/// 拖拽导入的根目录：设置显式指定 > 库根\Supervideo。
pub(crate) fn import_root_of(settings: &Settings, lib_root: &Path) -> PathBuf {
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
pub(crate) fn import_videos(state: State<'_, AppState>, app: AppHandle, paths: Vec<String>) -> Result<usize, String> {
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
pub(crate) fn import_status() -> Result<crate::importer::ImportStatus, String> {
    Ok(importer::status())
}

/// 取消导入：当前文件的半成品清掉，队列清空，已完成的保留。
#[tauri::command]
pub(crate) fn cancel_import() -> Result<(), String> {
    importer::cancel();
    Ok(())
}
