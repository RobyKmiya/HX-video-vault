//! `dedupe` 命令组，从 lib.rs 拆出；逻辑未改。
use super::prelude::*;

// ── 内容查重 ──────────────────────────────────────────────────────────

/// 后台算指纹。每批 12 个，算完放锁，避免长时间占着全局状态锁让 UI 卡。
/// 大头文件优先（按 size 倒序取批），先出结果的正好是空间占用最大的那些。
#[tauri::command]
pub(crate) fn dedupe_scan(app: AppHandle) -> Result<(), String> {
    // 先把 AppState 从 AppHandle 里摘出来（含 Arc），别让借用跟着进线程
    let st: AppState = app.state::<AppState>().inner().clone();
    {
        let ctx = st.lock()?;
        if ctx.dedupe_running.load(Ordering::SeqCst) {
            return Ok(());
        }
        ctx.dedupe_running.store(true, Ordering::SeqCst);
    }
    std::thread::spawn(move || {
        loop {
            let batch = {
                let Ok(ctx) = st.lock() else { break };
                let Ok(lib) = library(&ctx) else { break };
                match db::videos_needing_fingerprint(&lib.conn, 12) {
                    Ok(v) => v,
                    Err(_) => break,
                }
            };
            if batch.is_empty() {
                break;
            }
            for (id, path, size) in &batch {
                let fp = media::fingerprint(Path::new(path), *size as u64)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "!missing".to_string());
                if let Ok(ctx) = st.lock() {
                    if let Ok(lib) = library(&ctx) {
                        let _ = db::set_fingerprint(&lib.conn, *id, &fp);
                    }
                }
            }
            if let Ok(ctx) = st.lock() {
                if let Ok(lib) = library(&ctx) {
                    if let Ok((done, total)) = db::fingerprint_progress(&lib.conn) {
                        if let Ok(mut p) = ctx.dedupe_progress.lock() {
                            *p = (done, total);
                        }
                    }
                }
            }
            let _ = app.emit("dedupe-progress", serde_json::json!({ "event": "tick" }));
        }
        if let Ok(ctx) = st.lock() {
            ctx.dedupe_running.store(false, Ordering::SeqCst);
        }
        let _ = app.emit("dedupe-progress", serde_json::json!({ "event": "done" }));
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn dedupe_status(state: State<'_, AppState>) -> Result<crate::models::FingerprintProgress, String> {
    let ctx = state.lock()?;
    let running = ctx.dedupe_running.load(Ordering::SeqCst);
    let cached = ctx.dedupe_progress.lock().map(|p| *p).unwrap_or((0, 0));
    // 任务没在跑时直接读库，保证数字是当前真实的
    let (done, total) = if running {
        cached
    } else {
        let lib = library(&ctx)?;
        db::fingerprint_progress(&lib.conn).unwrap_or(cached)
    };
    Ok(crate::models::FingerprintProgress { done, total, running })
}

#[tauri::command]
pub(crate) fn dedupe_groups(state: State<'_, AppState>) -> Result<Vec<crate::models::DuplicateGroup>, String> {
    let ctx = state.lock()?;
    db::duplicate_groups(&library(&ctx)?.conn)
}

/// 把重复副本送进回收站（不删文件）。keep 为空时按每组第一条建议保留。
#[tauri::command]
pub(crate) fn dedupe_apply(state: State<'_, AppState>) -> Result<usize, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let groups = db::duplicate_groups(&lib.conn)?;
    let mut total = 0usize;
    for g in groups {
        let drops: Vec<i64> = g.others.iter().map(|i| i.id).collect();
        total += db::mark_duplicates(&lib.conn, g.keep.id, &drops)?;
    }
    Ok(total)
}

/// 按前端传来的选择精确处理：{ keep_id: [drop_id...] }
#[tauri::command]
pub(crate) fn dedupe_apply_selected(
    state: State<'_, AppState>,
    pairs: Vec<(i64, Vec<i64>)>,
) -> Result<usize, String> {
    let ctx = state.lock()?;
    let lib = library(&ctx)?;
    let mut total = 0usize;
    for (keep, drops) in pairs {
        total += db::mark_duplicates(&lib.conn, keep, &drops)?;
    }
    Ok(total)
}

/// 清掉回收站里文件已经不在的幽灵行
#[tauri::command]
pub(crate) fn purge_missing_trashed(state: State<'_, AppState>) -> Result<usize, String> {
    let ctx = state.lock()?;
    db::purge_missing_trashed(&library(&ctx)?.conn)
}
