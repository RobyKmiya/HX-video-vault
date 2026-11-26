//! `covers` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

pub fn set_contact_sheet(conn: &Connection, id: i64, path: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE videos SET contact_sheet=?2 WHERE id=?1",
        params![id, path],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
/// 记录用户自选封面（优先于接触表展示）。同一条重复设置即覆盖。
pub fn set_cover(conn: &Connection, id: i64, path: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO video_covers (video_id, path, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(video_id) DO UPDATE SET path=excluded.path, created_at=excluded.created_at",
        params![id, path, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
/// 撤销自选封面，回到接触表 / 中间帧兜底。
pub fn clear_cover(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM video_covers WHERE video_id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn get_cover_path(conn: &Connection, id: i64) -> Result<Option<String>, String> {
    conn.query_row("SELECT path FROM video_covers WHERE video_id=?1", params![id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())
}
/// 还没有接触表的视频 id（批量生成用），只取未删除的。
pub fn ids_without_sheet(conn: &Connection, limit: usize) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id FROM videos WHERE trashed=0 AND contact_sheet='' ORDER BY added_at DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
/// 全部未删除视频的 id（缺图的排前面，方便「先补没有的」）。
pub fn all_video_ids_for_sheet(conn: &Connection, only_missing: bool) -> Result<Vec<i64>, String> {
    let sql = if only_missing {
        "SELECT id FROM videos WHERE trashed=0 AND contact_sheet='' ORDER BY added_at DESC"
    } else {
        "SELECT id FROM videos WHERE trashed=0 ORDER BY (contact_sheet='') DESC, added_at DESC"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
/// 预览图统计：(总数, 已有, 缺图)。
pub fn sheet_counts(conn: &Connection) -> Result<(i64, i64, i64), String> {
    conn.query_row(
        "SELECT COUNT(*), SUM(CASE WHEN contact_sheet<>'' THEN 1 ELSE 0 END), SUM(CASE WHEN contact_sheet='' THEN 1 ELSE 0 END)
         FROM videos WHERE trashed=0",
        [],
        |r| {
            let total: i64 = r.get(0)?;
            let done: Option<i64> = r.get(1)?;
            let pending: Option<i64> = r.get(2)?;
            Ok((total, done.unwrap_or(0), pending.unwrap_or(0)))
        },
    )
    .map_err(|e| e.to_string())
}
/// 清空预览图记录，返回需要删除的图片路径。`ids=None` 表示全库。
/// 只清库里的字段，物理文件交给调用方删（这样清库失败不会留下半截状态）。
pub fn clear_contact_sheets(conn: &Connection, ids: Option<&[i64]>) -> Result<Vec<String>, String> {
    let mut paths: Vec<String> = Vec::new();
    match ids {
        Some(list) => {
            for id in list {
                let p: Option<String> = conn
                    .query_row("SELECT contact_sheet FROM videos WHERE id=?1", params![id], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?;
                if let Some(p) = p.filter(|s| !s.is_empty()) {
                    paths.push(p);
                }
                conn.execute("UPDATE videos SET contact_sheet='' WHERE id=?1", params![id])
                    .map_err(|e| e.to_string())?;
            }
        }
        None => {
            let mut stmt = conn
                .prepare("SELECT contact_sheet FROM videos WHERE contact_sheet<>''")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for p in rows.filter_map(Result::ok) {
                if !p.is_empty() {
                    paths.push(p);
                }
            }
            conn.execute("UPDATE videos SET contact_sheet='' WHERE contact_sheet<>''", [])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(paths)
}
