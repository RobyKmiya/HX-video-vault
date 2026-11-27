//! 壁纸导出助手命令组（影匣内托管 + 面板操控）。
use crate::we_export;

/// 面板一次性拿全：是否存活 / pid / node / 配置 / 最近日志。
#[tauri::command]
pub(crate) fn we_export_status() -> we_export::Status {
    we_export::status()
}

/// 拉起助手（幂等，已在跑就不动）并启动守护。
#[tauri::command]
pub(crate) fn we_export_start() {
    we_export::start();
}

/// 停掉助手。
#[tauri::command]
pub(crate) fn we_export_stop() {
    we_export::stop();
}

/// 自检：返回 /health（库根、区间夹数量等），失败给可读错误。
#[tauri::command]
pub(crate) fn we_export_test() -> Result<serde_json::Value, String> {
    we_export::health()
}

/// 改单个配置项（白名单内），立即落盘，下次导出生效。
#[tauri::command]
pub(crate) fn we_export_set_config(key: String, value: serde_json::Value) -> Result<(), String> {
    we_export::set_config(&key, value)
}
