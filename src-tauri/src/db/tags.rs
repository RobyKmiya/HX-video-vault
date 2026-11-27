//! `tags` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

pub fn refresh_tag_text(conn: &Connection, video_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE videos SET tag_text=COALESCE((SELECT GROUP_CONCAT(t.name, ' ') FROM video_tags vt JOIN tags t ON t.id=vt.tag_id WHERE vt.video_id=?1), '') WHERE id=?1",
        params![video_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn list_tags(conn: &Connection) -> Result<Vec<Tag>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.name, t.category, t.weight, COUNT(vt.video_id) AS cnt
             FROM tags t LEFT JOIN video_tags vt ON vt.tag_id=t.id
             GROUP BY t.id ORDER BY t.category='' DESC, t.category ASC, t.weight DESC, t.name ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                category: r.get(2)?,
                weight: r.get(3)?,
                count: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
pub fn create_tag(conn: &Connection, name: &str, category: &str, weight: i64) -> Result<i64, String> {
    conn.execute(
        "INSERT OR IGNORE INTO tags (name, category, weight, created_at) VALUES (?1,?2,?3,?4)",
        params![name.trim(), category.trim(), weight, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT id FROM tags WHERE name=?1", params![name.trim()], |r| r.get(0))
        .map_err(|e| e.to_string())
}
pub fn update_tag(conn: &Connection, id: i64, name: &str, category: &str, weight: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE tags SET name=?2, category=?3, weight=?4 WHERE id=?1",
        params![id, name.trim(), category.trim(), weight],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn set_tag_video(conn: &Connection, video_id: i64, tag_id: i64, add: bool) -> Result<(), String> {
    if add {
        conn.execute(
            "INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1,?2)",
            params![video_id, tag_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "DELETE FROM video_tags WHERE video_id=?1 AND tag_id=?2",
            params![video_id, tag_id],
        )
        .map_err(|e| e.to_string())?;
    }
    refresh_tag_text(conn, video_id)
}
/// 批量打标：返回实际影响条数。
pub fn bulk_tag(
    conn: &Connection,
    video_ids: &[i64],
    tag_ids: &[i64],
    add: bool,
) -> Result<usize, String> {
    let mut changed = 0usize;
    for vid in video_ids {
        for tid in tag_ids {
            let n = if add {
                conn.execute(
                    "INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1,?2)",
                    params![vid, tid],
                )
            } else {
                conn.execute(
                    "DELETE FROM video_tags WHERE video_id=?1 AND tag_id=?2",
                    params![vid, tid],
                )
            }
            .map_err(|e| e.to_string())?;
            changed += n;
        }
        refresh_tag_text(conn, *vid)?;
    }
    Ok(changed)
}
