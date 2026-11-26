//! `covers` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

// ---------- 封面 ----------

/// 封面生成的串行闸门：卡片首屏可能一次要几十张中帧，
/// 全放出去就是几十个 ffmpeg 进程；抽单帧本来就快，排队零伤害。
static COVER_GATE: Mutex<()> = Mutex::new(());

pub(crate) fn cover_gate() -> std::sync::MutexGuard<'static, ()> {
    COVER_GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 取一条视频的最佳封面路径：自选封面 > 接触表之外的中帧兜底（懒生成 + 永久缓存）。
/// 卡片只在既无自选封面又无接触表时才调用；压缩包条目会先解缓存再抽帧。
#[tauri::command]
pub(crate) async fn video_cover(state: State<'_, AppState>, id: i64) -> Result<String, String> {
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
pub(crate) async fn set_cover_image(
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
pub(crate) fn mpv_cover_path(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let dir = db::LibraryContext::vault_dir(&lib.root).join("cache").join("cover");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("cover-{id}.png")).to_string_lossy().to_string())
}

/// 内置播放器「设为封面」：canvas 在 asset 协议下会被污染导不出图（SecurityError），
/// 所以改由前端只报当前秒数，后端 ffmpeg 精确抽那一帧（输入侧 seek + accurate_seek）。
#[tauri::command]
pub(crate) async fn set_cover_from_time(state: State<'_, AppState>, id: i64, sec: f64) -> Result<(), String> {
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
pub(crate) async fn set_cover_from_mpv(state: State<'_, AppState>, id: i64, path: String) -> Result<(), String> {
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
pub(crate) fn clear_cover(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    db::clear_cover(&lib.conn, id)
}

/// 重置一个收藏夹（文件夹）内所有成员的缓存图：清自选封面记录 + 接触表记录，
/// 删 cover-{id}/mid-{id}/接触表缓存文件；返回被重置的成员 id 列表（前端据此给 URL 换版本号，
/// 防 WebView 继续用旧图）。前端随后刷新列表，卡片懒加载只对这个夹重新生成封面（不用全库慢扫）。
#[tauri::command]
pub(crate) fn reset_folder_covers(state: State<'_, AppState>, collection_id: i64) -> Result<Vec<i64>, String> {
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
