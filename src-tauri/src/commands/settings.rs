//! `settings` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

#[tauri::command]
pub(crate) fn app_settings(state: State<'_, AppState>) -> Result<(Settings, Option<String>, Option<LibraryInfo>), String> {
    let ctx = state.lock()?;
    let info = match &ctx.library {
        Some(lib) => Some(db::library_info(lib)?),
        None => None,
    };
    Ok((ctx.settings.clone(), ctx.startup_error.clone(), info))
}

#[tauri::command]
pub(crate) fn set_library(state: State<'_, AppState>, app: AppHandle, root: String) -> Result<LibraryInfo, String> {
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
pub(crate) fn update_settings(state: State<'_, AppState>, app: AppHandle, settings: Settings) -> Result<(), String> {
    let mut ctx = state.lock()?;
    media::set_tool_paths(settings.ffmpeg_path.as_deref(), settings.ffprobe_path.as_deref());
    ctx.settings = settings;
    write_settings(&app, &ctx.settings)
}

#[tauri::command]
pub(crate) async fn pick_directory(app: AppHandle) -> Result<Option<String>, String> {
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
pub(crate) fn probe_all(app: &AppHandle, paths: &[PathBuf]) -> Vec<db::VideoRecord> {
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
pub(crate) async fn scan(
    state: State<'_, AppState>,
    app: AppHandle,
    opts: ScanOptions,
) -> Result<ScanReport, String> {
    let state = state.inner().clone();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 扫描前先把散落/旧夹里的视频收进编号夹（1、2、3…每夹 200），再走目录，
        // 保证「扫描」同时也是「整理」，下载夹不会越堆越乱。
        {
            let guard = state.lock()?;
            if let Ok(lib) = library(&guard) {
                if db::needs_organize(&lib.root) {
                    let _ = db::organize_numbered(lib, db::FOLDER_CAP);
                }
            }
        }
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
pub(crate) fn stats(state: State<'_, AppState>) -> Result<Stats, String> {
    let ctx = state.lock()?;
    db::stats(&library(&ctx)?.conn)
}
