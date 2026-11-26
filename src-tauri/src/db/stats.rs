//! `stats` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

pub fn stats(conn: &Connection) -> Result<Stats, String> {
    let val = |sql: &str| -> Result<i64, String> {
        conn.query_row(sql, [], |r| r.get(0)).map_err(|e| e.to_string())
    };
    // 压缩包里的 N 集共享同一个容器文件大小，不能 N 倍计入；
    // 时长是每集各自的，直接求和。
    let total_duration: f64 = conn
        .query_row("SELECT COALESCE(SUM(duration_sec),0) FROM videos WHERE trashed=0", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let total_bytes: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(sz),0) FROM (SELECT path, MAX(size_bytes) AS sz FROM videos WHERE trashed=0 GROUP BY path)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut daily: Vec<DayCount> = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT substr(datetime(added_at,'unixepoch','localtime'),1,10) AS day, COUNT(*) AS cnt
             FROM videos WHERE trashed=0 AND added_at>0 GROUP BY day ORDER BY day DESC LIMIT 14",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(DayCount {
                day: r.get(0)?,
                count: r.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        daily.push(row.map_err(|e| e.to_string())?);
    }
    let mut tag_stmt = conn
        .prepare(
            "SELECT t.id, t.name, t.category, t.weight, COUNT(vt.video_id) AS cnt
             FROM tags t JOIN video_tags vt ON vt.tag_id=t.id
             GROUP BY t.id ORDER BY cnt DESC, t.name ASC LIMIT 30",
        )
        .map_err(|e| e.to_string())?;
    let tag_rows = tag_stmt
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
    let mut top_tags: Vec<Tag> = Vec::new();
    for row in tag_rows {
        top_tags.push(row.map_err(|e| e.to_string())?);
    }
    Ok(Stats {
        video_count: val("SELECT COUNT(*) FROM videos WHERE trashed=0")?,
        archive_count: val("SELECT COUNT(DISTINCT path) FROM videos WHERE trashed=0")?,
        total_bytes,
        total_duration_sec: total_duration,
        watched_count: val("SELECT COUNT(*) FROM videos WHERE trashed=0 AND watch_count>0")?,
        completed_count: val("SELECT COUNT(*) FROM videos WHERE trashed=0 AND completed=1")?,
        favorite_count: val("SELECT COUNT(*) FROM videos WHERE trashed=0 AND favorite=1")?,
        tag_count: val("SELECT COUNT(*) FROM tags")?,
        recent_days: daily,
        top_tags,
    })
}
/// 观看历史报表（最近 N 条），用于统计面板「观看记录」。
pub fn watch_history(conn: &Connection, limit: usize) -> Result<Vec<WatchRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT h.id, h.video_id, v.file_name, h.watched_at, h.from_sec, h.to_sec,
                    v.duration_sec, h.finished
             FROM watch_history h JOIN videos v ON v.id=h.video_id
             ORDER BY h.watched_at DESC, h.id DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64], |r| {
            Ok(WatchRecord {
                id: r.get(0)?,
                video_id: r.get(1)?,
                file_name: r.get(2)?,
                watched_at: r.get(3)?,
                from_sec: r.get(4)?,
                to_sec: r.get(5)?,
                duration_sec: r.get(6)?,
                finished: r.get::<_, i64>(7)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
