//! `downloads` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

pub fn list_downloads(conn: &Connection, limit: usize) -> Result<Vec<DownloadTask>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {DOWNLOAD_COLUMNS} FROM download_tasks ORDER BY created_at DESC LIMIT ?1"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([limit as i64], map_download).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
pub fn get_download(conn: &Connection, id: i64) -> Result<DownloadTask, String> {
    conn.query_row(
        &format!("SELECT {DOWNLOAD_COLUMNS} FROM download_tasks WHERE id=?1"),
        params![id],
        map_download,
    )
    .map_err(|e| e.to_string())
}
#[allow(clippy::too_many_arguments)]
pub fn create_download(
    conn: &Connection,
    url: &str,
    title: &str,
    site: &str,
    target_path: &str,
    tags: &str,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO download_tasks (url, title, site, state, target_path, tags, created_at, updated_at) VALUES (?1,?2,?3,'queued',?4,?5,?6,?6)",
        params![url, title, site, target_path, tags, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT last_insert_rowid()", [], |r| r.get(0)).map_err(|e| e.to_string())
}
pub fn update_download(
    conn: &Connection,
    id: i64,
    state: &str,
    progress: f64,
    error: &str,
    bytes_done: i64,
    bytes_total: i64,
    target_path: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE download_tasks SET state=?2, progress=?3, error=?4, bytes_done=?5, bytes_total=?6, updated_at=?7, target_path=COALESCE(?8,target_path) WHERE id=?1",
        params![id, state, progress, error, bytes_done, bytes_total, now_secs(), target_path],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn delete_download(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM download_tasks WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
/// 联网恢复时把中断的任务重新排队（进程被关掉时 running 状态会残留）。
pub fn requeue_interrupted(conn: &Connection) -> Result<usize, String> {
    conn.execute(
        "UPDATE download_tasks SET state='queued', error='' WHERE state IN ('running','paused')",
        [],
    )
    .map_err(|e| e.to_string())
}
