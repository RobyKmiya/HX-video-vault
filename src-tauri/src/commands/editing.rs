//! `editing` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

// ---------- 剪辑 ----------

/// 裁剪一段视频。素材可以是库内条目（含压缩包内条目）或任意本地文件。
#[tauri::command]
pub(crate) async fn trim_video(
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
pub(crate) async fn concat_video(
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
pub(crate) fn edit_status(state: State<'_, AppState>) -> Result<models::EditStatus, String> {
    let slot = state.lock()?.edit_status.clone();
    slot.lock()
        .map(|s| s.clone())
        .map_err(|_| "剪辑状态已损坏，请重启影匣".into())
}

/// 请求取消当前剪辑任务：ffmpeg 在下一行进度到来时被杀掉，半成品会被清掉。
#[tauri::command]
pub(crate) fn cancel_edit(state: State<'_, AppState>) -> Result<(), String> {
    let ctx = state.lock()?;
    if !ctx.edit_running.load(Ordering::SeqCst) {
        return Err("没有正在进行的剪辑任务".into());
    }
    ctx.edit_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 多选本地视频文件（外来素材入口）。
#[tauri::command]
pub(crate) async fn pick_video_files(app: AppHandle) -> Result<Vec<String>, String> {
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
pub(crate) fn probe_media(path: String) -> Result<crate::media::MediaMeta, String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("文件不存在".into());
    }
    Ok(media::probe(&p))
}
/// 把所选视频打包成一个 zip（**保留原文件**），落在第一条所在目录，返回压缩包路径。
/// 压缩包内条目（entry_index>0）跳过 —— 不能再套一层。
#[tauri::command]
pub(crate) fn compress_videos(
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
