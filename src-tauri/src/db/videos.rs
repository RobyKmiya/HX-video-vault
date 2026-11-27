//! `videos` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::*;

pub fn all_stamps(
    conn: &Connection,
) -> Result<std::collections::HashMap<(String, i64), VideoStamp>, String> {
    let mut stmt = conn
        .prepare("SELECT path, entry_index, size_bytes, mtime, duration_sec, width, height, entry_count FROM videos")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, i64>(1)?),
                VideoStamp {
                    size_bytes: r.get(2)?,
                    mtime: r.get(3)?,
                    has_meta: r.get::<_, f64>(4)? > 0.0
                        || (r.get::<_, i64>(5)? > 0 && r.get::<_, i64>(6)? > 0),
                    entry_count: r.get(7)?,
                },
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (k, v) = row.map_err(|e| e.to_string())?;
        out.insert(k, v);
    }
    Ok(out)
}
/// 按 (path, entry_index) 幂等登记；返回 true 表示新增。
/// 新记录没带元数据时保留库内已有的元数据（扫描不会把探测结果抹掉）。
///
/// 回收站标记只在「文件本身变了」时才清掉：扫描看到同一个路径 ≠ 用户想恢复它。
/// 以前这里无条件 `trashed=0`，导致每跑一次全库扫描就把回收站（含去重标记的副本）
/// 全部复活，去重成果被无声撤销。改判据后：内容/时间没变 → 尊重用户的回收站决定；
/// 文件被替换或重编码 → 视为新内容，正常参与浏览。要主动恢复用 `set_trashed(false)`。
pub fn upsert_video(conn: &Connection, rec: &VideoRecord) -> Result<bool, String> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM videos WHERE path=?1 AND entry_index=?2",
            params![rec.path, rec.entry_index],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match exists {
        Some(id) => {
            conn.execute(
                "UPDATE videos SET file_name=?2, container=?3, size_bytes=?4, mtime=?5,
                    duration_sec = CASE WHEN ?6 > 0 THEN ?6 ELSE duration_sec END,
                    width        = CASE WHEN ?7 > 0 THEN ?7 ELSE width END,
                    height       = CASE WHEN ?8 > 0 THEN ?8 ELSE height END,
                    codec_video  = CASE WHEN ?9 <> '' THEN ?9 ELSE codec_video END,
                    codec_audio  = CASE WHEN ?10 <> '' THEN ?10 ELSE codec_audio END,
                    entry_count=?11,
                    trashed = CASE WHEN size_bytes <> ?4 OR mtime <> ?5 THEN 0 ELSE trashed END
                    WHERE id=?1",
                params![id, rec.file_name, rec.container, rec.size_bytes, rec.mtime, rec.duration_sec, rec.width, rec.height, rec.codec_video, rec.codec_audio, rec.entry_count],
            ).map_err(|e| e.to_string())?;
            Ok(false)
        }
        None => {
            conn.execute(
                // import_seq 是「第几个被收进影匣的视频」：取当前最大值 +1，删掉的号不复用、也不回填空洞，
                // 这样它永远只增不改，能当导入顺序的身份证用。
                "INSERT INTO videos (path, file_name, container, size_bytes, mtime, duration_sec, width, height, codec_video, codec_audio, entry_index, entry_count, added_at, import_seq)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13, (SELECT COALESCE(MAX(import_seq),0)+1 FROM videos))",
                params![rec.path, rec.file_name, rec.container, rec.size_bytes, rec.mtime, rec.duration_sec, rec.width, rec.height, rec.codec_video, rec.codec_audio, rec.entry_index, rec.entry_count, now_secs()],
            ).map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
}
/// 彻底删除一条记录（关联的标签/收藏/封面/观看历史由外键级联清掉）。
/// 只动库，不碰磁盘文件——文件由调用方先删。
pub fn delete_video_row(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM videos WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_video(conn: &Connection, id: i64) -> Result<VideoItem, String> {
    let sql = format!("SELECT {VIDEO_COLUMNS} FROM videos v WHERE v.id=?1");
    conn.query_row(&sql, params![id], map_video).map_err(|e| e.to_string())
}
pub fn query_videos(conn: &Connection, q: &VideoQuery) -> Result<VideoPage, String> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(48).clamp(1, 200);
    let mut where_parts: Vec<String> = Vec::new();
    let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    match q.view.as_deref().unwrap_or("all") {
        "trash" => where_parts.push("v.trashed=1".into()),
        "favorites" => where_parts.push("v.favorite=1 AND v.trashed=0".into()),
        "recent" => where_parts.push("v.last_watched_at>0 AND v.trashed=0".into()),
        "latest" => where_parts.push("v.trashed=0".into()),
        "unwatched" => where_parts.push("v.watch_count=0 AND v.trashed=0".into()),
        "completed" => where_parts.push("v.completed=1 AND v.trashed=0".into()),
        "watchlist" => {
            where_parts.push("v.trashed=0 AND EXISTS (SELECT 1 FROM collection_videos cv JOIN collections c ON c.id=cv.collection_id WHERE cv.video_id=v.id AND c.kind='system' AND c.name='待看')".into());
        }
        _ => where_parts.push("v.trashed=0".into()),
    }
    if let Some(search) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        where_parts.push("(v.file_name LIKE ? ESCAPE '\\' OR v.path LIKE ? ESCAPE '\\' OR v.note LIKE ? ESCAPE '\\' OR v.tag_text LIKE ? ESCAPE '\\')".into());
        let pat = format!("%{}%", escape_like(search));
        for _ in 0..4 {
            binds.push(Box::new(pat.clone()));
        }
    }
    if let Some(tag) = q.tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        where_parts.push("EXISTS (SELECT 1 FROM video_tags vt JOIN tags t ON t.id=vt.tag_id WHERE vt.video_id=v.id AND t.name=? )".into());
        binds.push(Box::new(tag.to_string()));
    }
    let mut collection_name: Option<String> = None;
    if let Some(coll) = q.collection.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        where_parts.push("EXISTS (SELECT 1 FROM collection_videos cv JOIN collections c ON c.id=cv.collection_id WHERE cv.video_id=v.id AND c.name=? AND c.deleted_at=0 )".into());
        binds.push(Box::new(coll.to_string()));
        collection_name = Some(coll.to_string());
    }
    let where_sql = where_parts.join(" AND ");
    // 排序键可带 - 前缀表示逆序（如 -added / -mtime / -title）；manual = 收藏夹内拖拽位次；
    // ORDER BY 的绑定参数要排在 WHERE 参数之后、LIMIT 之前
    // 无前缀 = 各键的既有默认方向（added/size/watched/count/duration/rating/mtime 新近在前，
    // title/manual 升序）；- 前缀取反。老调用不传前缀，行为与历史版本一致。
    let raw_sort = q.sort.as_deref().unwrap_or("added").to_string();
    let default_desc = !matches!(raw_sort.trim_start_matches('-'), "title" | "manual");
    let (key, desc) = match raw_sort.strip_prefix('-') {
        Some(k) => (k.to_string(), !default_desc),
        None => (raw_sort, default_desc),
    };
    let mut order_binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let order = match key.as_str() {
        "manual" => {
            if let Some(name) = collection_name.clone() {
                order_binds.push(Box::new(name));
                format!(
                    "(SELECT cv2.position FROM collection_videos cv2 JOIN collections c2 ON c2.id=cv2.collection_id WHERE cv2.video_id=v.id AND c2.name=? AND c2.deleted_at=0) {}, v.added_at {}",
                    if desc { "DESC" } else { "ASC" },
                    if desc { "DESC" } else { "ASC" }
                )
            } else {
                added_order(desc)
            }
        }
        "title" => format!("v.file_name COLLATE NOCASE {}", if desc { "DESC" } else { "ASC" }),
        "folder" => format!("v.path COLLATE NOCASE {}", if desc { "DESC" } else { "ASC" }),
        "size" => format!("v.size_bytes {}", if desc { "DESC" } else { "ASC" }),
        "mtime" => format!("v.mtime {}", if desc { "DESC" } else { "ASC" }),
        "watched" => format!("v.last_watched_at {}", if desc { "DESC" } else { "ASC" }),
        "count" => format!("v.watch_count {}", if desc { "DESC" } else { "ASC" }),
        "duration" => format!("v.duration_sec {}", if desc { "DESC" } else { "ASC" }),
        "rating" => format!("v.rating {}", if desc { "DESC" } else { "ASC" }),
        _ => added_order(desc),
    };
    // 「最近入库」是固定语义的平铺视图：跨所有文件夹按真实入库顺序看，
    // 不受 sort 下拉影响，进不了文件夹也看不到手动位次。
    let order = if q.view.as_deref() == Some("latest") {
        added_order(true)
    } else {
        order
    };
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        binds.iter().map(|b| b.as_ref()).collect();
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM videos v WHERE {where_sql}"),
            params_ref.as_slice(),
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let offset = (page - 1) * page_size;
    let mut page_binds = binds;
    page_binds.extend(order_binds);
    page_binds.push(Box::new(page_size as i64));
    page_binds.push(Box::new(offset as i64));
    let page_refs: Vec<&dyn rusqlite::types::ToSql> =
        page_binds.iter().map(|b| b.as_ref()).collect();
    let sql = format!(
        "SELECT {VIDEO_COLUMNS} FROM videos v WHERE {where_sql} ORDER BY {order} LIMIT ?{} OFFSET ?{}",
        page_binds.len() - 1,
        page_binds.len()
    );
    // map_video 用列名取值，SELECT 列表里的列没有 v. 前缀，可直接用。
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(page_refs.as_slice(), map_video)
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| e.to_string())?);
    }
    Ok(VideoPage {
        items,
        total,
        page,
        page_size,
    })
}
pub fn patch_video(conn: &Connection, p: &VideoPatch) -> Result<(), String> {
    let mut sets: Vec<String> = Vec::new();
    let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    macro_rules! set {
        ($field:literal, $val:expr) => {{
            sets.push(format!("{}=?{}", $field, binds.len() + 1));
            binds.push(Box::new($val));
        }};
    }
    if let Some(v) = p.rating {
        set!("rating", v);
    }
    if let Some(v) = &p.note {
        set!("note", v.clone());
    }
    if let Some(v) = p.favorite {
        set!("favorite", v as i64);
    }
    if let Some(v) = p.completed {
        set!("completed", v as i64);
    }
    if let Some(v) = p.progress_sec {
        set!("progress_sec", v);
    }
    if sets.is_empty() {
        return Ok(());
    }
    binds.push(Box::new(p.id));
    let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    conn.execute(
        &format!("UPDATE videos SET {} WHERE id=?{}", sets.join(","), binds.len()),
        refs.as_slice(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
/// 记一次观看：累计次数、更新进度、写入历史。重复打开同一位置时不会虚增次数。
pub fn record_watch(conn: &Connection, u: &WatchUpdate) -> Result<(), String> {
    let current: (f64, i64, f64) = conn
        .query_row(
            "SELECT progress_sec, watch_count, duration_sec FROM videos WHERE id=?1",
            params![u.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    let duration = if u.duration_sec > 0.0 { u.duration_sec } else { current.2 };
    let finished = u.completed.unwrap_or(false)
        || (duration > 0.0 && u.progress_sec >= duration * 0.95);
    // 只有真正前进了才算一次新的观看会话（避免反复 seek 把次数刷爆）
    let new_session = current.1 == 0
        || u.progress_sec < current.0 * 0.5
        || u.progress_sec <= 1.0;
    let count = current.1 + if new_session { 1 } else { 0 };
    conn.execute(
        "UPDATE videos SET progress_sec=?2, watch_count=?3, last_watched_at=?4, completed=CASE WHEN ?5=1 THEN 1 ELSE completed END WHERE id=?1",
        params![u.id, u.progress_sec, count, now_secs(), finished as i64],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO watch_history (video_id, watched_at, from_sec, to_sec, finished) VALUES (?1,?2,?3,?4,?5)",
        params![u.id, now_secs(), current.0, u.progress_sec, finished as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn set_trashed(conn: &Connection, id: i64, trashed: bool) -> Result<(), String> {
    conn.execute(
        "UPDATE videos SET trashed=?2 WHERE id=?1",
        params![id, trashed as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
/// 回填「见到才知道」的元数据。压缩包内条目在扫描时只读中央目录、不解压，
/// 所以时长/分辨率是空的；等它第一次被打开或抽帧时顺手补上。
/// 只在原值为空时写入，绝不覆盖已有数据。
pub fn backfill_probe(
    conn: &Connection,
    id: i64,
    duration_sec: f64,
    width: i64,
    height: i64,
    codec_video: &str,
    codec_audio: &str,
) -> Result<bool, String> {
    if duration_sec <= 0.0 && width <= 0 && height <= 0 {
        return Ok(false);
    }
    let n = conn
        .execute(
            "UPDATE videos SET
                duration_sec = CASE WHEN duration_sec > 0 THEN duration_sec ELSE ?2 END,
                width        = CASE WHEN width > 0 THEN width ELSE ?3 END,
                height       = CASE WHEN height > 0 THEN height ELSE ?4 END,
                codec_video  = CASE WHEN codec_video <> '' THEN codec_video ELSE ?5 END,
                codec_audio  = CASE WHEN codec_audio <> '' THEN codec_audio ELSE ?6 END
             WHERE id = ?1",
            params![id, duration_sec, width, height, codec_video, codec_audio],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}
/// 还没有时长信息的条目 id（绝大多数是压缩包内条目）。
pub fn ids_without_meta(conn: &Connection, limit: usize) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM videos WHERE trashed=0 AND duration_sec<=0 ORDER BY id LIMIT ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}
/// 一条视频的源路径与包内序号（改名/移动/定位前取原始位置）。
pub fn video_source(conn: &Connection, id: i64) -> Result<(String, i64), String> {
    conn.query_row("SELECT path, entry_index FROM videos WHERE id=?1", params![id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })
    .map_err(|e| format!("找不到这条视频：{e}"))
}
/// 文件被改名/移动后同步库：更新 path / file_name / container / mtime，
/// 并把自动镜像夹的成员关系从旧文件夹搬到新文件夹（新夹不存在就建，旧夹空了就删）。
/// 只碰这一条记录，**不做全库扫描**——这是「改名移动不卡」的关键。
pub fn apply_path_change(
    conn: &Connection,
    root: &Path,
    video_id: i64,
    new_path: &Path,
) -> Result<(), String> {
    let old_path: String = conn
        .query_row("SELECT path FROM videos WHERE id=?1", params![video_id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let new_str = new_path.to_string_lossy().to_string();
    let file_name = new_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let container = new_path
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mtime = crate::fileops::mtime_secs(new_path);

    conn.execute(
        "UPDATE videos SET path=?2, file_name=?3, container=?4, mtime=CASE WHEN ?5>0 THEN ?5 ELSE mtime END
         WHERE id=?1",
        params![video_id, new_str, file_name, container, mtime],
    )
    .map_err(|e| e.to_string())?;

    let old_dir = rel_dir_of(root, &old_path);
    let new_dir = rel_dir_of(root, &new_str);
    if old_dir == new_dir {
        return Ok(()); // 同目录改名：夹不变
    }

    if let Some(name) = old_dir.as_deref() {
        if let Ok(cid) = auto_collection_id(conn, name) {
            conn.execute(
                "DELETE FROM collection_videos WHERE collection_id=?1 AND video_id=?2",
                params![cid, video_id],
            )
            .map_err(|e| e.to_string())?;
            prune_empty_auto(conn, cid)?;
        }
    }
    if let Some(name) = new_dir.as_deref() {
        let cid = ensure_auto_collection(conn, name)?;
        conn.execute(
            "INSERT OR IGNORE INTO collection_videos (collection_id, video_id, added_at, position)
             VALUES (?1, ?2, ?3,
                     COALESCE((SELECT MAX(position)+1 FROM collection_videos WHERE collection_id=?1), 0))",
            params![cid, video_id, now_secs()],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// 侧栏「库根」与所有 auto 文件夹，供移动对话框选择目标（按名字排序）。
pub fn list_move_targets(conn: &Connection, root: &Path) -> Result<Vec<(String, String, i64)>, String> {
    let root_str = root.to_string_lossy().to_string();
    let mut stmt = conn
        .prepare(
            "SELECT c.name, COUNT(cv.video_id)
             FROM collections c LEFT JOIN collection_videos cv ON cv.collection_id=c.id
             WHERE c.kind='auto' AND c.deleted_at=0
             GROUP BY c.id ORDER BY c.name",
        )
        .map_err(|e| e.to_string())?;
    let mut out: Vec<(String, String, i64)> = Vec::new();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows.filter_map(Result::ok) {
        let (name, count) = row;
        let abs = format!("{}{}{}", root_str, std::path::MAIN_SEPARATOR, name.replace(" › ", &std::path::MAIN_SEPARATOR.to_string()));
        out.push((name, abs, count));
    }
    Ok(out)
}
/// 标记扫描中已不存在的文件（不删除数据库记录，进「失效」状态由用户决定）。
pub fn mark_missing(conn: &Connection, seen_paths: &[String]) -> Result<usize, String> {
    if seen_paths.is_empty() {
        return Ok(0);
    }
    // 用临时表传路径，避免 SQL 参数上限
    conn.execute("CREATE TEMP TABLE IF NOT EXISTS _seen (path TEXT PRIMARY KEY)", [])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM _seen", []).map_err(|e| e.to_string())?;
    {
        let mut stmt = conn.prepare("INSERT INTO _seen (path) VALUES (?1)").map_err(|e| e.to_string())?;
        for p in seen_paths {
            stmt.execute(params![p]).map_err(|e| e.to_string())?;
        }
    }
    let n = conn
        .execute(
            "UPDATE videos SET trashed=1 WHERE trashed=0 AND path NOT IN (SELECT path FROM _seen)",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(n)
}
/// 库内所有不重复的文件路径（供扫描去重 / 后续 CLI 使用）。
#[allow(dead_code)]
pub fn known_paths(conn: &Connection) -> Result<std::collections::HashSet<String>, String> {
    let mut stmt = conn.prepare("SELECT DISTINCT path FROM videos").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
    let mut set = std::collections::HashSet::new();
    for row in rows {
        set.insert(row.map_err(|e| e.to_string())?);
    }
    Ok(set)
}

// ---------- 下载任务 ----------
