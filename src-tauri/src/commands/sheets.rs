//! `sheets` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

/// 为一条视频生成接触表。抽帧是 CPU 密集活，放到阻塞线程池。
#[tauri::command]
pub(crate) async fn make_contact_sheet(state: State<'_, AppState>, id: i64) -> Result<String, String> {
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
pub(crate) async fn make_contact_sheets(
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
pub(crate) fn cancel_contact_sheets(state: State<'_, AppState>) -> Result<(), String> {
    let ctx = state.lock()?;
    ctx.sheet_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 清空预览图记录（回滚）。`ids=None` 时清全库；返回实际清掉的图片数量。
#[tauri::command]
pub(crate) fn clear_contact_sheets(state: State<'_, AppState>, ids: Option<Vec<i64>>) -> Result<usize, String> {
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
pub(crate) fn sheet_status(state: State<'_, AppState>) -> Result<crate::models::SheetStatus, String> {
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
