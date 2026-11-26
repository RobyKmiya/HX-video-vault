//! `dedupe` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

/// 取一批还没算指纹的活跃视频（id, path, size）。按大小倒序，
/// 大文件先算——它们才是空间大头，早点出结果早点能清理。
pub fn videos_needing_fingerprint(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(i64, String, i64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, path, size_bytes FROM videos
             WHERE trashed=0 AND fingerprint='' AND size_bytes>0
             ORDER BY size_bytes DESC, id ASC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}
/// 写入一条指纹。文件读不出来（掉盘、被删）时写 `!missing` 占位，
/// 免得下一轮又把它捞出来反复重试。
pub fn set_fingerprint(conn: &Connection, id: i64, fp: &str) -> Result<(), String> {
    conn.execute("UPDATE videos SET fingerprint=?1 WHERE id=?2", params![fp, id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
/// 指纹进度：(已算, 待算总数)
pub fn fingerprint_progress(conn: &Connection) -> Result<(i64, i64), String> {
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM videos WHERE trashed=0 AND size_bytes>0 AND fingerprint NOT LIKE '!%'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let done: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM videos WHERE trashed=0 AND size_bytes>0 AND fingerprint<>'' AND fingerprint NOT LIKE '!%'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok((done, total))
}
/// 找内容重复的组。保留策略：看过次数最多的那份，其次看得最近的，
/// 再次入库最早的——用户已经看过的副本留在原地，省得进度一起丢掉。
pub fn duplicate_groups(conn: &Connection) -> Result<Vec<DuplicateGroup>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT fingerprint, COUNT(*) FROM videos
             WHERE trashed=0 AND fingerprint<>'' AND fingerprint NOT LIKE '!%'
             GROUP BY fingerprint HAVING COUNT(*)>1
             ORDER BY SUM(size_bytes) DESC",
        )
        .map_err(|e| e.to_string())?;
    let groups: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let mut out = Vec::new();
    for (fp, _) in groups {
        let mut inner = conn
            .prepare(
                "SELECT id, path, file_name, size_bytes, watch_count, last_watched_at, added_at
                 FROM videos WHERE trashed=0 AND fingerprint=?1
                 ORDER BY watch_count DESC, last_watched_at DESC, added_at ASC, id ASC",
            )
            .map_err(|e| e.to_string())?;
        let mut items: Vec<DuplicateItem> = inner
            .query_map(params![fp], dup_item_from_row)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        if items.len() < 2 {
            continue;
        }
        let keep = items.remove(0);
        let wasted_bytes = items.iter().map(|i| i.size_bytes).sum();
        let size_bytes = keep.size_bytes;
        out.push(DuplicateGroup { fingerprint: fp, size_bytes, keep, others: items, wasted_bytes });
    }
    Ok(out)
}
/// 把重复副本标记进回收站（不删文件，留给用户在回收站确认后自己删）。
/// 关联的标签/收藏夹关系转到保留项上，进度不丢。
pub fn mark_duplicates(conn: &Connection, keep_id: i64, drop_ids: &[i64]) -> Result<usize, String> {
    if drop_ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let now = now_secs();
    let mut n = 0usize;
    for dup in drop_ids {
        if *dup == keep_id {
            continue;
        }
        // 保留项不在收藏夹里、副本在的话，把副本的归属转过去
        tx.execute(
            "INSERT OR IGNORE INTO collection_videos(collection_id, video_id, added_at, position)
             SELECT collection_id, ?1, added_at, position FROM collection_videos WHERE video_id=?2",
            params![keep_id, dup],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO video_tags(video_id, tag_id) SELECT ?1, tag_id FROM video_tags WHERE video_id=?2",
            params![keep_id, dup],
        )
        .map_err(|e| e.to_string())?;
        let changed = tx
            .execute(
                "UPDATE videos SET trashed=1, dedup_of=?1, last_watched_at=COALESCE(NULLIF(last_watched_at,0), ?2)
                 WHERE id=?3 AND trashed=0",
                params![keep_id, now, dup],
            )
            .map_err(|e| e.to_string())?;
        n += changed;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}
/// 清掉「回收站里但文件已经不在盘上」的幽灵行，返回删掉的行数。
/// 删文件没清库会攒出这种行：统计面板显示的回收站占用是假的。
pub fn purge_missing_trashed(conn: &Connection) -> Result<usize, String> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, path FROM videos WHERE trashed=1")
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect()
    };
    let dead: Vec<i64> = rows
        .into_iter()
        .filter(|(_, p)| !Path::new(p).is_file())
        .map(|(id, _)| id)
        .collect();
    if dead.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for id in &dead {
        tx.execute("DELETE FROM videos WHERE id=?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(dead.len())
}
