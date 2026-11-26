//! `collections` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

/// 列出目录。`include_deleted=true` 时把软删除的也带上（回收站视图用）。
pub fn list_collections(conn: &Connection, include_deleted: bool) -> Result<Vec<Collection>, String> {
    let where_sql = if include_deleted { "" } else { "WHERE c.deleted_at=0" };
    let sql = format!(
        "SELECT c.id, c.name, c.kind, COUNT(cv.video_id) AS cnt, c.group_name, c.deleted_at
         FROM collections c LEFT JOIN collection_videos cv ON cv.collection_id=c.id
         {where_sql}
         GROUP BY c.id ORDER BY c.kind='custom', c.group_name, c.sort_order, c.id"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Collection {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                count: r.get(3)?,
                group_name: r.get(4)?,
                deleted_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
/// 新建目录。同名且已被软删除的，直接复活并挂到新分组，避免 UNIQUE 冲突后静默失败。
pub fn create_collection(conn: &Connection, name: &str, group_name: &str) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("目录名不能为空".into());
    }
    let group_name = group_name.trim();
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, deleted_at FROM collections WHERE name=?1 AND kind='custom'",
            params![name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((id, deleted_at)) = existing {
        if deleted_at > 0 {
            conn.execute(
                "UPDATE collections SET deleted_at=0, group_name=?2 WHERE id=?1",
                params![id, group_name],
            )
            .map_err(|e| e.to_string())?;
        }
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO collections (name, kind, created_at, group_name) VALUES (?1, 'custom', ?2, ?3)",
        params![name, now_secs(), group_name],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}
pub fn rename_collection(conn: &Connection, id: i64, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("目录名不能为空".into());
    }
    let kind: String = conn
        .query_row("SELECT kind FROM collections WHERE id=?1", params![id], |r| r.get(0))
        .map_err(|e| format!("目录不存在: {e}"))?;
    if kind == "system" {
        return Err("收藏 / 待看是内置目录，不能改名".into());
    }
    let dup: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE name=?1 AND kind='custom' AND id<>?2",
            params![name, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if dup.is_some() {
        return Err(format!("已经有同名目录「{name}」"));
    }
    conn.execute("UPDATE collections SET name=?2 WHERE id=?1", params![id, name])
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn set_collection_group(conn: &Connection, id: i64, group_name: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE collections SET group_name=?2 WHERE id=?1",
        params![id, group_name.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
/// 软删除：只标记，视频与目录的关联全部保留，可在回收站恢复。
pub fn soft_delete_collection(conn: &Connection, id: i64) -> Result<(), String> {
    let kind: String = conn
        .query_row("SELECT kind FROM collections WHERE id=?1", params![id], |r| r.get(0))
        .map_err(|e| format!("目录不存在: {e}"))?;
    if kind == "system" {
        return Err("收藏 / 待看是内置目录，不能删除".into());
    }
    conn.execute(
        "UPDATE collections SET deleted_at=?2 WHERE id=?1",
        params![id, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn restore_collection(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("UPDATE collections SET deleted_at=0 WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
/// 彻底删除（只在回收站里由用户确认后调用）。先清关联再删目录。
pub fn purge_collection(conn: &Connection, id: i64) -> Result<(), String> {
    let kind: String = conn
        .query_row("SELECT kind FROM collections WHERE id=?1", params![id], |r| r.get(0))
        .map_err(|e| format!("目录不存在: {e}"))?;
    if kind == "system" {
        return Err("收藏 / 待看是内置目录，不能删除".into());
    }
    conn.execute("DELETE FROM collection_videos WHERE collection_id=?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM collections WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn set_collection_video(
    conn: &Connection,
    collection_id: i64,
    video_id: i64,
    add: bool,
) -> Result<(), String> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE id=?1 AND deleted_at=0",
            params![collection_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if exists.is_none() {
        return Err("目录不存在或已删除".into());
    }
    if add {
        conn.execute(
            "INSERT OR IGNORE INTO collection_videos (collection_id, video_id, added_at, position)
             VALUES (?1, ?2, ?3, COALESCE((SELECT MAX(position)+1 FROM collection_videos WHERE collection_id=?1), 0))",
            params![collection_id, video_id, now_secs()],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "DELETE FROM collection_videos WHERE collection_id=?1 AND video_id=?2",
            params![collection_id, video_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// 按名字取目录 id（内置「待看」这类固定入口用）。
pub fn collection_id_by_name(conn: &Connection, name: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT id FROM collections WHERE name=?1 AND deleted_at=0",
        params![name],
        |r| r.get(0),
    )
    .map_err(|e| format!("目录「{name}」不存在: {e}"))
}
/// 侧栏收藏夹拖拽排序：按前端给出的完整 id 顺序落 sort_order。
pub fn reorder_collections(conn: &Connection, ids: &[i64]) -> Result<(), String> {
    for (idx, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE collections SET sort_order=?2 WHERE id=?1",
            params![id, idx as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// 收藏夹内视频拖拽排序。前端只传当前可见子集的新顺序：
/// 未出现在子集里的成员整体后移 subset 长度（保持原有相对次序），避免位次冲突。
pub fn reorder_collection_videos(
    conn: &Connection,
    collection_id: i64,
    video_ids: &[i64],
) -> Result<(), String> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE id=?1 AND deleted_at=0",
            params![collection_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if exists.is_none() {
        return Err("收藏夹不存在或已删除".into());
    }
    let shift = video_ids.len() as i64;
    conn.execute(
        "UPDATE collection_videos SET position=position+?2
         WHERE collection_id=?1 AND video_id NOT IN (SELECT value FROM json_each(?3))",
        params![collection_id, shift, serde_json::to_string(video_ids).map_err(|e| e.to_string())?],
    )
    .map_err(|e| e.to_string())?;
    for (idx, vid) in video_ids.iter().enumerate() {
        conn.execute(
            "UPDATE collection_videos SET position=?3 WHERE collection_id=?1 AND video_id=?2",
            params![collection_id, vid, idx as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
