use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::models::{
    Collection, DayCount, LibraryInfo, Stats, Tag, VideoItem, VideoPage, VideoPatch, VideoQuery,
    WatchRecord, WatchUpdate,
};

pub const SCHEMA_VERSION: i64 = 1;

/// 与 `video` 表一一对应的写入载荷（不带 id / 标签）。
#[derive(Debug, Clone)]
pub struct VideoRecord {
    pub path: String,
    pub file_name: String,
    pub container: String,
    pub size_bytes: i64,
    pub mtime: i64,
    pub duration_sec: f64,
    pub width: i64,
    pub height: i64,
    pub codec_video: String,
    pub codec_audio: String,
    pub entry_index: i64,
    pub entry_count: i64,
}

pub struct LibraryContext {
    pub root: PathBuf,
    #[allow(dead_code)] // 供后续 vaultctl CLI 与外部备份工具使用
    pub db_path: PathBuf,
    pub conn: Connection,
}

impl LibraryContext {
    pub fn vault_dir(root: &Path) -> PathBuf {
        root.join(".vault")
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 本地时间（UTC+8）文件名友好格式，避免为一个格式化函数引入 chrono。
fn file_stamp() -> String {
    let total = now_secs() + 8 * 3600;
    let days = total.div_euclid(86_400);
    let secs = total.rem_euclid(86_400);
    // Howard Hinnant 的 civil_from_days，days 自 1970-01-01 起
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 { year += 1; }
    format!(
        "{year:04}{month:02}{day:02}-{h:02}{m:02}{s:02}",
        h = secs / 3600,
        m = (secs % 3600) / 60,
        s = secs % 60,
    )
}

fn map_video(row: &Row<'_>) -> rusqlite::Result<VideoItem> {
    let tag_text: String = row.get("tag_text")?;
    let tags = tag_text
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let coll_raw: Option<String> = row.get("collections")?;
    let collections = coll_raw
        .unwrap_or_default()
        .split('\u{1f}')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Ok(VideoItem {
        id: row.get("id")?,
        path: row.get("path")?,
        file_name: row.get("file_name")?,
        container: row.get("container")?,
        size_bytes: row.get("size_bytes")?,
        mtime: row.get("mtime")?,
        duration_sec: row.get("duration_sec")?,
        width: row.get("width")?,
        height: row.get("height")?,
        codec_video: row.get("codec_video")?,
        codec_audio: row.get("codec_audio")?,
        entry_index: row.get("entry_index")?,
        entry_count: row.get("entry_count")?,
        added_at: row.get("added_at")?,
        last_watched_at: row.get("last_watched_at")?,
        watch_count: row.get("watch_count")?,
        progress_sec: row.get("progress_sec")?,
        completed: row.get::<_, i64>("completed")? != 0,
        rating: row.get("rating")?,
        note: row.get("note")?,
        favorite: row.get::<_, i64>("favorite")? != 0,
        trashed: row.get::<_, i64>("trashed")? != 0,
        contact_sheet: row.get("contact_sheet")?,
        tag_text,
        tags,
        collections,
    })
}

const VIDEO_COLUMNS: &str = "id, path, file_name, container, size_bytes, mtime, duration_sec, width, height, codec_video, codec_audio, entry_index, entry_count, added_at, last_watched_at, watch_count, progress_sec, completed, rating, note, favorite, trashed, contact_sheet, tag_text, (SELECT GROUP_CONCAT(c.name, char(31)) FROM collection_videos cv JOIN collections c ON c.id=cv.collection_id WHERE cv.video_id=v.id) AS collections";

/// 打开（必要时创建）一个影匣库。库数据放在 <root>/.vault/vault.db，
/// 与用户视频文件同盘但不混入媒体目录。
pub fn open_library(root: &Path) -> Result<LibraryContext, String> {
    fs::create_dir_all(root).map_err(|e| format!("无法创建库目录: {e}"))?;
    let vault = LibraryContext::vault_dir(root);
    fs::create_dir_all(&vault).map_err(|e| format!("无法创建 .vault: {e}"))?;
    fs::create_dir_all(vault.join("cache")).map_err(|e| format!("无法创建缓存目录: {e}"))?;
    let db_path = vault.join("vault.db");
    let conn = Connection::open(&db_path).map_err(|e| format!("无法打开数据库: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| e.to_string())?;
    migrate(&conn)?;
    daily_backup(root, &db_path);
    Ok(LibraryContext {
        root: root.to_path_buf(),
        db_path,
        conn,
    })
}

fn migrate(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if version > SCHEMA_VERSION {
        return Err(format!(
            "库版本 {version} 高于本程序支持的 {SCHEMA_VERSION}，请升级影匣"
        ));
    }
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS videos (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path TEXT NOT NULL,
  file_name TEXT NOT NULL,
  container TEXT NOT NULL DEFAULT '',
  size_bytes INTEGER NOT NULL DEFAULT 0,
  mtime INTEGER NOT NULL DEFAULT 0,
  duration_sec REAL NOT NULL DEFAULT 0,
  width INTEGER NOT NULL DEFAULT 0,
  height INTEGER NOT NULL DEFAULT 0,
  codec_video TEXT NOT NULL DEFAULT '',
  codec_audio TEXT NOT NULL DEFAULT '',
  entry_index INTEGER NOT NULL DEFAULT 0,
  entry_count INTEGER NOT NULL DEFAULT 1,
  added_at INTEGER NOT NULL DEFAULT 0,
  last_watched_at INTEGER NOT NULL DEFAULT 0,
  watch_count INTEGER NOT NULL DEFAULT 0,
  progress_sec REAL NOT NULL DEFAULT 0,
  completed INTEGER NOT NULL DEFAULT 0,
  rating INTEGER NOT NULL DEFAULT 0,
  note TEXT NOT NULL DEFAULT '',
  favorite INTEGER NOT NULL DEFAULT 0,
  trashed INTEGER NOT NULL DEFAULT 0,
  contact_sheet TEXT NOT NULL DEFAULT '',
  tag_text TEXT NOT NULL DEFAULT '',
  UNIQUE(path, entry_index)
);
CREATE INDEX IF NOT EXISTS idx_videos_path ON videos(path);
CREATE INDEX IF NOT EXISTS idx_videos_added ON videos(added_at DESC);
CREATE INDEX IF NOT EXISTS idx_videos_watched ON videos(last_watched_at DESC);
CREATE INDEX IF NOT EXISTS idx_videos_size ON videos(size_bytes DESC);

CREATE TABLE IF NOT EXISTS tags (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE,
  category TEXT NOT NULL DEFAULT '',
  weight INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS video_tags (
  video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (video_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_vt_tag ON video_tags(tag_id);

CREATE TABLE IF NOT EXISTS collections (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'custom',
  created_at INTEGER NOT NULL DEFAULT 0,
  UNIQUE(name, kind)
);
CREATE TABLE IF NOT EXISTS collection_videos (
  collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  added_at INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (collection_id, video_id)
);

CREATE TABLE IF NOT EXISTS watch_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  watched_at INTEGER NOT NULL,
  from_sec REAL NOT NULL DEFAULT 0,
  to_sec REAL NOT NULL DEFAULT 0,
  finished INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_wh_video ON watch_history(video_id);
CREATE INDEX IF NOT EXISTS idx_wh_time ON watch_history(watched_at DESC);

CREATE TABLE IF NOT EXISTS download_tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  url TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  site TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL DEFAULT 'queued',
  progress REAL NOT NULL DEFAULT 0,
  target_path TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL DEFAULT 0,
  error TEXT NOT NULL DEFAULT ''
);
",
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO collections (name, kind) VALUES ('收藏', 'system'), ('待看', 'system')",
        [],
    )
    .map_err(|e| e.to_string())?;
    if version < SCHEMA_VERSION {
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 每日一致性快照，滚动保留 3 份（画匣 A2 的同款保护，视频库更值钱）。
fn daily_backup(root: &Path, db_path: &Path) {
    let backup_dir = LibraryContext::vault_dir(root).join("backups");
    if fs::create_dir_all(&backup_dir).is_err() {
        return;
    }
    let mut existing: Vec<(SystemTime, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&backup_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("db") {
                if let Ok(meta) = fs::metadata(&p) {
                    if let Ok(created) = meta.created() {
                        existing.push((created, p));
                    }
                }
            }
        }
    }
    existing.sort_by_key(|(t, _)| *t);
    while existing.len() >= 3 {
        let (_, old) = existing.remove(0);
        let _ = fs::remove_file(old);
    }
    let newest = existing.last().and_then(|(t, _)| t.elapsed().ok());
    if newest.is_some_and(|d| d < Duration::from_secs(20 * 3600)) {
        return;
    }
    let name = format!("vault-{}.db", file_stamp());
    let target = backup_dir.join(name);
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return;
    };
    let sql = format!("VACUUM INTO '{}'", target.to_string_lossy().replace('\'', "''"));
    let _ = conn.execute_batch(&sql);
}

pub fn library_info(ctx: &LibraryContext) -> Result<LibraryInfo, String> {
    let count: i64 = ctx
        .conn
        .query_row("SELECT COUNT(*) FROM videos WHERE trashed=0", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(LibraryInfo {
        root: ctx.root.to_string_lossy().to_string(),
        video_count: count,
        schema_version: SCHEMA_VERSION,
    })
}

/// 按 (path, entry_index) 幂等登记；返回 true 表示新增。
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
                "UPDATE videos SET file_name=?2, container=?3, size_bytes=?4, mtime=?5, duration_sec=?6, width=?7, height=?8, codec_video=?9, codec_audio=?10, entry_count=?11, trashed=0 WHERE id=?1",
                params![id, rec.file_name, rec.container, rec.size_bytes, rec.mtime, rec.duration_sec, rec.width, rec.height, rec.codec_video, rec.codec_audio, rec.entry_count],
            ).map_err(|e| e.to_string())?;
            Ok(false)
        }
        None => {
            conn.execute(
                "INSERT INTO videos (path, file_name, container, size_bytes, mtime, duration_sec, width, height, codec_video, codec_audio, entry_index, entry_count, added_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![rec.path, rec.file_name, rec.container, rec.size_bytes, rec.mtime, rec.duration_sec, rec.width, rec.height, rec.codec_video, rec.codec_audio, rec.entry_index, rec.entry_count, now_secs()],
            ).map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
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
    if let Some(coll) = q.collection.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        where_parts.push("EXISTS (SELECT 1 FROM collection_videos cv JOIN collections c ON c.id=cv.collection_id WHERE cv.video_id=v.id AND c.name=? )".into());
        binds.push(Box::new(coll.to_string()));
    }
    let where_sql = where_parts.join(" AND ");
    let order = match q.sort.as_deref().unwrap_or("added") {
        "title" => "v.file_name COLLATE NOCASE ASC",
        "size" => "v.size_bytes DESC",
        "watched" => "v.last_watched_at DESC",
        "count" => "v.watch_count DESC",
        "duration" => "v.duration_sec DESC",
        "rating" => "v.rating DESC",
        _ => "v.added_at DESC",
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

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
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

pub fn set_contact_sheet(conn: &Connection, id: i64, path: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE videos SET contact_sheet=?2 WHERE id=?1",
        params![id, path],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn refresh_tag_text(conn: &Connection, video_id: i64) -> Result<(), String> {
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

pub fn list_collections(conn: &Connection) -> Result<Vec<Collection>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, c.kind, COUNT(cv.video_id) AS cnt
             FROM collections c LEFT JOIN collection_videos cv ON cv.collection_id=c.id
             GROUP BY c.id ORDER BY c.kind='custom', c.name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Collection {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                count: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn create_collection(conn: &Connection, name: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO collections (name, kind, created_at) VALUES (?1, 'custom', ?2)",
        params![name.trim(), now_secs()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_collection_video(
    conn: &Connection,
    name: &str,
    video_id: i64,
    add: bool,
) -> Result<(), String> {
    let id: i64 = conn
        .query_row("SELECT id FROM collections WHERE name=?1", params![name], |r| r.get(0))
        .map_err(|e| format!("目录「{name}」不存在: {e}"))?;
    if add {
        conn.execute(
            "INSERT OR IGNORE INTO collection_videos (collection_id, video_id, added_at) VALUES (?1,?2,?3)",
            params![id, video_id, now_secs()],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "DELETE FROM collection_videos WHERE collection_id=?1 AND video_id=?2",
            params![id, video_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_lib() -> (PathBuf, LibraryContext) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("vv-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ctx = open_library(&root).expect("临时库应能创建");
        (root, ctx)
    }

    fn rec(path: &str, entry: i64) -> VideoRecord {
        VideoRecord {
            path: path.into(),
            file_name: format!("视频-{entry}"),
            container: "mp4".into(),
            size_bytes: 1024,
            mtime: 100,
            duration_sec: 100.0,
            width: 1920,
            height: 1080,
            codec_video: "h264".into(),
            codec_audio: "aac".into(),
            entry_index: entry,
            entry_count: 3,
        }
    }

    #[test]
    fn schema_upsert_is_idempotent_per_entry() {
        let (root, ctx) = temp_lib();
        assert_eq!(upsert_video(&ctx.conn, &rec("D:\\a.zip", 0)).unwrap(), true);
        assert_eq!(upsert_video(&ctx.conn, &rec("D:\\a.zip", 1)).unwrap(), true);
        // 同一个 (path, entry_index) 再写一次应算更新
        assert_eq!(upsert_video(&ctx.conn, &rec("D:\\a.zip", 0)).unwrap(), false);
        let page = query_videos(&ctx.conn, &VideoQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].entry_count, 3);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tags_drive_search_and_tag_text() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("D:\\b.mp4", 0)).unwrap();
        let id = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get::<_, i64>(0)).unwrap();
        let tag_id = create_tag(&ctx.conn, "风景", "题材", 2).unwrap();
        set_tag_video(&ctx.conn, id, tag_id, true).unwrap();
        let one = get_video(&ctx.conn, id).unwrap();
        assert_eq!(one.tags, vec!["风景"]);
        assert!(one.tag_text.contains("风景"));
        let q = VideoQuery { search: Some("风景".into()), ..Default::default() };
        assert_eq!(query_videos(&ctx.conn, &q).unwrap().total, 1);
        set_tag_video(&ctx.conn, id, tag_id, false).unwrap();
        assert_eq!(query_videos(&ctx.conn, &q).unwrap().total, 0);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn watch_count_and_progress_are_stable() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("D:\\c.mp4", 0)).unwrap();
        let id = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get::<_, i64>(0)).unwrap();
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 30.0, duration_sec: 100.0, completed: None }).unwrap();
        assert_eq!(get_video(&ctx.conn, id).unwrap().watch_count, 1);
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 60.0, duration_sec: 100.0, completed: None }).unwrap();
        assert_eq!(get_video(&ctx.conn, id).unwrap().watch_count, 1); // 连续看不算新会话
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 2.0, duration_sec: 100.0, completed: None }).unwrap();
        assert_eq!(get_video(&ctx.conn, id).unwrap().watch_count, 2); // 从头看算新会话
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 98.0, duration_sec: 100.0, completed: None }).unwrap();
        assert!(get_video(&ctx.conn, id).unwrap().completed);
        let s = stats(&ctx.conn).unwrap();
        assert_eq!(s.video_count, 1);
        assert_eq!(s.watched_count, 1);
        assert_eq!(s.completed_count, 1);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }
}
