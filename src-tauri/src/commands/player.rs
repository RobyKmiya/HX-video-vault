//! `player` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

#[tauri::command]
pub(crate) fn playback_info(state: State<'_, AppState>, id: i64) -> Result<PlaybackInfo, String> {
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
pub(crate) fn play_external(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let info = player::resolve(lib, id)?;
    let path = PathBuf::from(&info.media_path);
    if !path.is_file() {
        return Err("视频文件不存在".into());
    }
    player::launch_external(&path, info.start_sec)
}
#[tauri::command]
pub(crate) fn watch_history(state: State<'_, AppState>, limit: Option<usize>) -> Result<Vec<crate::models::WatchRecord>, String> {
    let ctx = state.lock()?;
    db::watch_history(&library(&ctx)?.conn, limit.unwrap_or(50).clamp(1, 500))
}
#[tauri::command]
pub(crate) fn player_engines(state: State<'_, AppState>) -> Result<Vec<mpv::EngineInfo>, String> {
    let ctx = state.lock()?;
    Ok(mpv::engines(ctx.settings.mpv_path.as_deref()))
}

/// 用 mpv IPC 打开一条视频。返回 true 表示已接管，false 表示调用方应退回内置播放器。
#[tauri::command]
pub(crate) async fn mpv_open(
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
pub(crate) fn mpv_command(state: State<'_, AppState>, action: String, value: Option<serde_json::Value>) -> Result<(), String> {
    let ctx = state.lock()?;
    let session = ctx.mpv.as_ref().ok_or("mpv 播放器未启动")?;
    let cmd = mpv::command_from(&action, value)?;
    session.send(cmd)
}

#[tauri::command]
pub(crate) fn mpv_close(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(session) = state.lock()?.mpv.take() {
        session.stop();
    }
    Ok(())
}
