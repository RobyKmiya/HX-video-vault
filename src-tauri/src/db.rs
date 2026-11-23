use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::models::{
    Collection, DayCount, DownloadTask, LibraryInfo, Stats, Tag, VideoItem, VideoPage, VideoPatch,
    VideoQuery, WatchRecord, WatchUpdate,
};

pub const SCHEMA_VERSION: i64 = 5;

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
        // LEFT JOIN 没命中时是 NULL，走 Option 再落成空串
        cover: row.get::<_, Option<String>>("cover")?.unwrap_or_default(),
        tag_text,
        tags,
        collections,
    })
}

const VIDEO_COLUMNS: &str = "id, path, file_name, container, size_bytes, mtime, duration_sec, width, height, codec_video, codec_audio, entry_index, entry_count, added_at, last_watched_at, watch_count, progress_sec, completed, rating, note, favorite, trashed, contact_sheet, tag_text, (SELECT path FROM video_covers vc WHERE vc.video_id = v.id) AS cover, (SELECT GROUP_CONCAT(c.name, char(31)) FROM collection_videos cv JOIN collections c ON c.id=cv.collection_id WHERE cv.video_id=v.id AND c.deleted_at=0) AS collections";

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
  deleted_at INTEGER NOT NULL DEFAULT 0,
  group_name TEXT NOT NULL DEFAULT '',
  sort_order INTEGER NOT NULL DEFAULT 0,
  UNIQUE(name, kind)
);
CREATE TABLE IF NOT EXISTS collection_videos (
  collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  added_at INTEGER NOT NULL DEFAULT 0,
  position INTEGER NOT NULL DEFAULT 0,
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

CREATE TABLE IF NOT EXISTS video_covers (
  video_id INTEGER PRIMARY KEY REFERENCES videos(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS download_tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  url TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  site TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL DEFAULT 'queued',
  progress REAL NOT NULL DEFAULT 0,
  target_path TEXT NOT NULL DEFAULT '',
  tags TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0,
  bytes_done INTEGER NOT NULL DEFAULT 0,
  bytes_total INTEGER NOT NULL DEFAULT 0,
  error TEXT NOT NULL DEFAULT ''
);
",
    )
    .map_err(|e| e.to_string())?;
    // v1 → v2：已存在的库补列（SQLite 的 ADD COLUMN 不支持 IF NOT EXISTS，用 pragma 探测）
    if version < 2 {
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(download_tasks)").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        for (name, ddl) in [
            ("tags", "ALTER TABLE download_tasks ADD COLUMN tags TEXT NOT NULL DEFAULT ''"),
            ("updated_at", "ALTER TABLE download_tasks ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0"),
            ("bytes_done", "ALTER TABLE download_tasks ADD COLUMN bytes_done INTEGER NOT NULL DEFAULT 0"),
            ("bytes_total", "ALTER TABLE download_tasks ADD COLUMN bytes_total INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !cols.iter().any(|c| c == name) {
                conn.execute_batch(ddl).map_err(|e| e.to_string())?;
            }
        }
    }
    conn.execute(
        "INSERT OR IGNORE INTO collections (name, kind) VALUES ('收藏', 'system'), ('待看', 'system')",
        [],
    )
    .map_err(|e| e.to_string())?;
    // v2 → v3：收藏集支持分组 / 软删除 / 手动排序
    if version < 3 {
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(collections)").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        for (name, ddl) in [
            ("deleted_at", "ALTER TABLE collections ADD COLUMN deleted_at INTEGER NOT NULL DEFAULT 0"),
            ("group_name", "ALTER TABLE collections ADD COLUMN group_name TEXT NOT NULL DEFAULT ''"),
            ("sort_order", "ALTER TABLE collections ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !cols.iter().any(|c| c == name) {
                conn.execute_batch(ddl).map_err(|e| e.to_string())?;
            }
        }
    }
    // v3 → v4 已在历史版本完成；v4 → v5：收藏夹成员支持手动排序 + 旧红心收藏迁入默认收藏夹
    if version < 5 {
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(collection_videos)").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        if !cols.iter().any(|c| c == "position") {
            conn.execute_batch("ALTER TABLE collection_videos ADD COLUMN position INTEGER NOT NULL DEFAULT 0")
                .map_err(|e| e.to_string())?;
        }
        // 红心收藏（favorite=1）一次性迁入默认收藏夹「我的收藏」：
        // 该夹已存在（含软删）时 INSERT OR IGNORE 得 0 变更，绝不动用户已有的同名夹
        let created = conn.execute(
            "INSERT OR IGNORE INTO collections (name, kind) VALUES ('我的收藏', 'custom')",
            [],
        )
        .map_err(|e| e.to_string())?;
        if created > 0 {
            let cid = conn.last_insert_rowid();
            conn.execute(
                "INSERT OR IGNORE INTO collection_videos (collection_id, video_id, added_at, position)
                 SELECT ?1, id, ?2, ROW_NUMBER() OVER (ORDER BY id) - 1 FROM videos WHERE favorite=1",
                params![cid, now_secs()],
            )
            .map_err(|e| e.to_string())?;
        }
    }
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

/// 扫描用：库内已登记条目的「轻量快照」，一次查全库，避免逐文件查库。
#[derive(Debug, Clone, Copy)]
pub struct VideoStamp {
    pub size_bytes: i64,
    pub mtime: i64,
    /// 时长/分辨率是否已经探测过（压缩包条目通常为 false）
    pub has_meta: bool,
    pub entry_count: i64,
}

impl VideoStamp {
    /// 文件没变且元数据齐全 → 这次扫描不必再跑 ffprobe。
    pub fn is_fresh(&self, size_bytes: i64, mtime: i64) -> bool {
        self.has_meta && self.size_bytes == size_bytes && self.mtime == mtime
    }
}

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
                    entry_count=?11, trashed=0 WHERE id=?1",
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
                format!("v.added_at {}", if desc { "DESC" } else { "ASC" })
            }
        }
        "title" => format!("v.file_name COLLATE NOCASE {}", if desc { "DESC" } else { "ASC" }),
        "size" => format!("v.size_bytes {}", if desc { "DESC" } else { "ASC" }),
        "mtime" => format!("v.mtime {}", if desc { "DESC" } else { "ASC" }),
        "watched" => format!("v.last_watched_at {}", if desc { "DESC" } else { "ASC" }),
        "count" => format!("v.watch_count {}", if desc { "DESC" } else { "ASC" }),
        "duration" => format!("v.duration_sec {}", if desc { "DESC" } else { "ASC" }),
        "rating" => format!("v.rating {}", if desc { "DESC" } else { "ASC" }),
        _ => format!("v.added_at {}", if desc { "DESC" } else { "ASC" }),
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

/// 主页层级浏览：列出某层下的 auto 子文件夹卡。
/// parent = None 取顶层（名字里没有 › 的单段夹）；Some(路径) 取以「路径 › 」开头的一层。
/// 封面拼贴素材取夹内前 4 个成员（按位次）的已有图片：自选封面 > 中帧缓存 > 接触表，
/// 现场不生成任何图（点进文件夹后视频卡片的懒加载会逐步把缓存填满，下一轮浏览自然有图）。
pub fn list_folder_children(
    conn: &Connection,
    root: &Path,
    parent: Option<&str>,
) -> Result<Vec<crate::models::FolderInfo>, String> {
    const SEP: &str = " › ";
    let (like_kw, bind): (&str, String) = match parent {
        None => ("NOT LIKE", format!("%{}%", SEP)),
        Some(p) => ("LIKE", format!("%{}{}%", escape_like(p), escape_like(SEP))),
    };
    let sql = format!(
        "SELECT c.id, c.name, COUNT(cv.video_id)
         FROM collections c LEFT JOIN collection_videos cv ON cv.collection_id = c.id
         WHERE c.kind='auto' AND c.deleted_at=0 AND c.name {like_kw} ? ESCAPE '\\'
         GROUP BY c.id ORDER BY c.name COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![bind], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(|e| e.to_string())?;

    let mut member_stmt = conn
        .prepare("SELECT video_id FROM collection_videos WHERE collection_id=?1 ORDER BY position LIMIT 4")
        .map_err(|e| e.to_string())?;
    let cover_dir = LibraryContext::vault_dir(root).join("cache").join("cover");

    let mut out = Vec::new();
    for (id, name, count) in rows.filter_map(Result::ok) {
        let leaf = match parent {
            None => name.clone(),
            Some(p) => name[p.len() + SEP.len()..].to_string(),
        };
        let mut covers: Vec<String> = Vec::new();
        let member_ids: Vec<i64> = member_stmt
            .query_map(params![id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        for vid in member_ids {
            if covers.len() >= 4 {
                break;
            }
            let candidates = [
                get_cover_path(conn, vid).ok().flatten(),
                Some(cover_dir.join(format!("mid-{vid}.jpg")).to_string_lossy().to_string()),
            ];
            for c in candidates.into_iter().flatten() {
                if covers.len() < 4 && Path::new(&c).is_file() {
                    covers.push(c);
                    break;
                }
            }
            if covers.len() < 4 {
                if let Ok(item) = get_video(conn, vid) {
                    if !item.contact_sheet.is_empty() && Path::new(&item.contact_sheet).is_file() {
                        covers.push(item.contact_sheet);
                    }
                }
            }
        }
        out.push(crate::models::FolderInfo { id, name, leaf, count, covers });
    }
    Ok(out)
}

/// 扫描后同步「磁盘文件夹 → 自动收藏夹」：
/// 库根下的每层子目录镜像成 kind='auto' 的收藏夹（名 = 相对路径以 › 连接，分组 = 首段），
/// 视频/压缩包条目按所在目录自动归位；文件移走或删掉的旧链接会被清掉。
/// 根目录直接放的文件不归类（无文件夹可镜像）。手动收藏夹（custom/system）不受影响。
/// 返回本次同步的文件夹数。
pub fn sync_folder_collections(
    conn: &Connection,
    root: &Path,
    seen_paths: &[String],
) -> Result<usize, String> {
    use std::collections::{HashMap, HashSet};

    let root_str = root.to_string_lossy().to_string();
    let seen: HashSet<&String> = seen_paths.iter().collect();

    // 本次在盘上的视频 id → 所在相对目录（空串 = 根目录，跳过）
    let mut dir_ids: HashMap<String, Vec<i64>> = HashMap::new();
    let mut dir_paths: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, path FROM videos")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for (vid, path) in rows.filter_map(Result::ok) {
            if !seen.contains(&path) {
                continue;
            }
            let rel = path
                .strip_prefix(&root_str)
                .map(|rest| rest.trim_start_matches(['\\', '/']))
                .unwrap_or("");
            let Some(idx) = rel.rfind(['\\', '/']) else { continue }; // 根目录直放：无文件夹
            let rel_dir = &rel[..idx];
            dir_ids.entry(rel_dir.to_string()).or_default().push(vid);
            dir_paths.entry(rel_dir.to_string()).or_default().push(path);
        }
    }

    let mut synced = 0usize;
    for (rel_dir, _ids) in &dir_ids {
        let name = rel_dir.replace(['\\', '/'], " › ");
        let group = name.split(" › ").next().unwrap_or("").to_string();
        conn.execute(
            "INSERT OR IGNORE INTO collections (name, kind, group_name) VALUES (?1, 'auto', ?2)",
            params![name, group],
        )
        .map_err(|e| e.to_string())?;
        let cid: i64 = conn
            .query_row(
                "SELECT id FROM collections WHERE name=?1 AND kind='auto'",
                params![name],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let paths_json = serde_json::to_string(&dir_paths[rel_dir]).map_err(|e| e.to_string())?;

        // 文件已移走/删除的旧链接清掉
        conn.execute(
            "DELETE FROM collection_videos
             WHERE collection_id=?1
               AND video_id NOT IN (SELECT id FROM videos WHERE path IN (SELECT value FROM json_each(?2)))",
            params![cid, paths_json],
        )
        .map_err(|e| e.to_string())?;

        // 缺的链接补上，位次排到当前夹尾
        conn.execute(
            "INSERT OR IGNORE INTO collection_videos (collection_id, video_id, added_at, position)
             SELECT ?1, v.id, ?2,
                    COALESCE((SELECT MAX(position)+1 FROM collection_videos WHERE collection_id=?1), 0)
                      + ROW_NUMBER() OVER (ORDER BY v.id) - 1
             FROM videos v
             WHERE v.path IN (SELECT value FROM json_each(?3))
               AND v.id NOT IN (SELECT video_id FROM collection_videos WHERE collection_id=?1)",
            params![cid, now_secs(), paths_json],
        )
        .map_err(|e| e.to_string())?;
        synced += 1;
    }

    // 已清空/已消失的文件夹：目录里已经没有视频 → 整夹清空并删除，
    // 否则残留链接永远挂在 side 栏（文件夹是唯一事实源，夹随盘走）
    let occupied: HashSet<String> = dir_ids
        .keys()
        .map(|d| d.replace(['\\', '/'], " › "))
        .collect();
    let mut stmt = conn
        .prepare("SELECT id, name FROM collections WHERE kind='auto'")
        .map_err(|e| e.to_string())?;
    let autos: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    for (cid, name) in autos {
        if !occupied.contains(&name) {
            conn.execute("DELETE FROM collection_videos WHERE collection_id=?1", params![cid])
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM collections WHERE id=?1", params![cid])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(synced)
}

/// 夹内全部成员 id（按位次）。
pub fn collection_member_ids(conn: &Connection, collection_id: i64) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT video_id FROM collection_videos WHERE collection_id=?1 ORDER BY position")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![collection_id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

// ---------- 应用内文件编辑：改名 / 移动后的库内同步 ----------

/// 绝对路径 → 相对库根的文件夹名（"A › B"）；库根直放或不在库里返回 None。
fn rel_dir_of(root: &Path, path: &str) -> Option<String> {
    let root_str = root.to_string_lossy();
    let rel = path.strip_prefix(root_str.as_ref())?.trim_start_matches(['\\', '/']);
    let idx = rel.rfind(['\\', '/'])?;
    if idx == 0 {
        return None;
    }
    Some(rel[..idx].replace(['\\', '/'], " › "))
}

fn auto_collection_id(conn: &Connection, name: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT id FROM collections WHERE name=?1 AND kind='auto'",
        params![name],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// 取（必要时建）某个目录对应的 auto 镜像夹；被软删过的一并复活。
fn ensure_auto_collection(conn: &Connection, name: &str) -> Result<i64, String> {
    let group = name.split(" › ").next().unwrap_or("").to_string();
    conn.execute(
        "INSERT OR IGNORE INTO collections (name, kind, group_name) VALUES (?1, 'auto', ?2)",
        params![name, group],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE collections SET deleted_at=0 WHERE name=?1 AND kind='auto'",
        params![name],
    )
    .map_err(|e| e.to_string())?;
    auto_collection_id(conn, name)
}

/// 目录夹空了就删掉（文件夹是唯一事实源：没视频的夹不该留在侧栏）。
fn prune_empty_auto(conn: &Connection, collection_id: i64) -> Result<(), String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM collection_videos WHERE collection_id=?1",
            params![collection_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        conn.execute(
            "DELETE FROM collections WHERE id=?1 AND kind='auto'",
            params![collection_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
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

const DOWNLOAD_COLUMNS: &str = "id, url, title, site, state, progress, target_path, tags, created_at, updated_at, error, bytes_done, bytes_total";

fn map_download(row: &Row<'_>) -> rusqlite::Result<DownloadTask> {
    Ok(DownloadTask {
        id: row.get(0)?,
        url: row.get(1)?,
        title: row.get(2)?,
        site: row.get(3)?,
        state: row.get(4)?,
        progress: row.get(5)?,
        target_path: row.get(6)?,
        tags: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        error: row.get(10)?,
        bytes_done: row.get(11)?,
        bytes_total: row.get(12)?,
    })
}

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
    fn stamps_flag_unchanged_entries_and_upsert_keeps_meta() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("D:\\c.mp4", 0)).unwrap();
        let id: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path=?1", params!["D:\\c.mp4"], |r| r.get(0))
            .unwrap();

        let stamps = all_stamps(&ctx.conn).unwrap();
        let st = stamps.get(&("D:\\c.mp4".to_string(), 0)).expect("应有戳记");
        assert!(st.is_fresh(1024, 100), "没变过且元数据齐全 → 不必再探测");
        assert!(!st.is_fresh(2048, 100), "大小变了要重新探测");
        assert!(!st.is_fresh(1024, 200), "mtime 变了要重新探测");

        // 扫描时写回的 stub 没有元数据，不能把已有元数据抹掉
        let mut stub = rec("D:\\c.mp4", 0);
        stub.duration_sec = 0.0;
        stub.width = 0;
        stub.height = 0;
        stub.codec_video = String::new();
        stub.codec_audio = String::new();
        stub.size_bytes = 2048;
        assert!(!upsert_video(&ctx.conn, &stub).unwrap());
        let after = get_video(&ctx.conn, id).unwrap();
        assert_eq!(after.duration_sec, 100.0);
        assert_eq!(after.width, 1920);
        assert_eq!(after.size_bytes, 2048);

        // 从没探测过的条目 → has_meta=false，仍然需要探测
        let mut bare = rec("D:\\d.mp4", 0);
        bare.duration_sec = 0.0;
        bare.width = 0;
        bare.height = 0;
        upsert_video(&ctx.conn, &bare).unwrap();
        let stamps = all_stamps(&ctx.conn).unwrap();
        assert!(!stamps[&("D:\\d.mp4".to_string(), 0)].is_fresh(1024, 100));

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
    fn download_task_crud_and_requeue() {
        let (root, ctx) = temp_lib();
        let id = create_download(&ctx.conn, "https://x/a.mp4", "a.mp4", "x.com", "/tmp/a.mp4", "标签A").unwrap();
        let t = get_download(&ctx.conn, id).unwrap();
        assert_eq!(t.state, "queued");
        assert_eq!(t.tags, "标签A");
        update_download(&ctx.conn, id, "running", 0.5, "", 50, 100, None).unwrap();
        assert_eq!(get_download(&ctx.conn, id).unwrap().progress, 0.5);
        // 中断后重排（模拟进程被关掉）
        assert_eq!(requeue_interrupted(&ctx.conn).unwrap(), 1);
        assert_eq!(get_download(&ctx.conn, id).unwrap().state, "queued");
        delete_download(&ctx.conn, id).unwrap();
        assert!(list_downloads(&ctx.conn, 10).unwrap().is_empty());
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

    /// 压缩包条目入库时时长是 0，等真正解出来再补；补过之后不能被后来的探测覆盖。
    #[test]
    fn backfill_fills_missing_meta_but_never_overwrites() {
        let (root, ctx) = temp_lib();
        let mut archive = rec("D:\\合集.cbz", 0);
        archive.duration_sec = 0.0;
        archive.width = 0;
        archive.height = 0;
        archive.codec_video = String::new();
        archive.codec_audio = String::new();
        upsert_video(&ctx.conn, &archive).unwrap();
        let id = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get::<_, i64>(0)).unwrap();

        // 入库时确实没有元数据
        assert_eq!(get_video(&ctx.conn, id).unwrap().duration_sec, 0.0);
        assert!(ids_without_meta(&ctx.conn, 10).unwrap().contains(&id));

        // 第一次解出来 → 补上
        assert!(backfill_probe(&ctx.conn, id, 1425.5, 1920, 1080, "h264", "aac").unwrap());
        let filled = get_video(&ctx.conn, id).unwrap();
        assert_eq!(filled.duration_sec, 1425.5);
        assert_eq!(filled.width, 1920);
        assert_eq!(filled.codec_video, "h264");
        assert!(!ids_without_meta(&ctx.conn, 10).unwrap().contains(&id));

        // 再来一次探测（值不同）→ 原有数据保持不变
        backfill_probe(&ctx.conn, id, 99.0, 320, 240, "hevc", "opus").unwrap();
        let again = get_video(&ctx.conn, id).unwrap();
        assert_eq!(again.duration_sec, 1425.5);
        assert_eq!(again.width, 1920);
        assert_eq!(again.codec_video, "h264");

        // 探测不到任何东西时不该写库
        assert!(!backfill_probe(&ctx.conn, id, 0.0, 0, 0, "", "").unwrap());
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 目录：分组、软删除、恢复、改名后关联不掉、彻底删除不动视频
    #[test]
    fn collections_support_group_soft_delete_and_restore() {
        let (root, ctx) = temp_lib();
        let id = create_collection(&ctx.conn, "  灵感  ", "创作").expect("建目录");
        assert_eq!(collection_id_by_name(&ctx.conn, "灵感").unwrap(), id);

        upsert_video(&ctx.conn, &rec("K:/a.mp4", 0)).unwrap();
        let vid: i64 = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get(0)).unwrap();
        set_collection_video(&ctx.conn, id, vid, true).unwrap();

        let mine = list_collections(&ctx.conn, false).unwrap().into_iter().find(|c| c.id == id).unwrap();
        assert_eq!(mine.group_name, "创作");
        assert_eq!(mine.count, 1);

        // 软删除：默认列表看不到，回收站看得到
        soft_delete_collection(&ctx.conn, id).unwrap();
        assert!(list_collections(&ctx.conn, false).unwrap().iter().all(|c| c.id != id));
        assert!(list_collections(&ctx.conn, true).unwrap().iter().any(|c| c.id == id && c.deleted_at > 0));
        assert!(get_video(&ctx.conn, vid).unwrap().collections.is_empty(), "已删目录不该出现在视频的目录里");

        // 恢复后关联仍在
        restore_collection(&ctx.conn, id).unwrap();
        assert_eq!(get_video(&ctx.conn, vid).unwrap().collections, vec!["灵感".to_string()]);

        // 内置目录受保护
        let fav = collection_id_by_name(&ctx.conn, "收藏").unwrap();
        assert!(soft_delete_collection(&ctx.conn, fav).is_err());
        assert!(purge_collection(&ctx.conn, fav).is_err());

        // 改名不影响关联（过滤/展示都按 id 走）
        rename_collection(&ctx.conn, id, "灵感库").unwrap();
        assert!(collection_id_by_name(&ctx.conn, "灵感").is_err());
        assert_eq!(collection_id_by_name(&ctx.conn, "灵感库").unwrap(), id);
        assert_eq!(get_video(&ctx.conn, vid).unwrap().collections, vec!["灵感库".to_string()]);

        // 同名冲突要被挡住，不能悄悄建出第二条
        create_collection(&ctx.conn, "另一个", "").unwrap();
        assert!(rename_collection(&ctx.conn, id, "另一个").is_err());

        // 彻底删除：目录没了，视频还在
        purge_collection(&ctx.conn, id).unwrap();
        assert!(list_collections(&ctx.conn, true).unwrap().iter().all(|c| c.id != id));
        assert!(get_video(&ctx.conn, vid).is_ok());

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 同名目录软删后再「新建」应复活原记录，而不是被 UNIQUE 挡掉
    #[test]
    fn creating_same_name_after_delete_revives_it() {
        let (root, ctx) = temp_lib();
        let a = create_collection(&ctx.conn, "素材", "").unwrap();
        soft_delete_collection(&ctx.conn, a).unwrap();
        let b = create_collection(&ctx.conn, "素材", "整理后").unwrap();
        assert_eq!(a, b);
        let c = list_collections(&ctx.conn, false).unwrap().into_iter().find(|c| c.id == a).unwrap();
        assert_eq!(c.group_name, "整理后");
        assert_eq!(c.deleted_at, 0);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 老库（user_version=2、collections 没有新列）打开时要自动补列，且数据不丢
    #[test]
    fn v2_library_gains_collection_columns_on_open() {
        let root = std::env::temp_dir().join(format!(
            "vv-mig-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let vault = LibraryContext::vault_dir(&root);
        fs::create_dir_all(&vault).unwrap();
        {
            let conn = Connection::open(vault.join("vault.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE collections (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   name TEXT NOT NULL,
                   kind TEXT NOT NULL DEFAULT 'custom',
                   created_at INTEGER NOT NULL DEFAULT 0,
                   UNIQUE(name, kind));
                 INSERT INTO collections (name, kind) VALUES ('旧目录', 'custom');
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }
        let ctx = open_library(&root).expect("老库应能迁移");
        let cols: Vec<String> = {
            let mut stmt = ctx.conn.prepare("PRAGMA table_info(collections)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.filter_map(Result::ok).collect()
        };
        for want in ["deleted_at", "group_name", "sort_order"] {
            assert!(cols.iter().any(|c| c == want), "迁移后缺少列 {want}");
        }
        let version: i64 = ctx.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let old = list_collections(&ctx.conn, false).unwrap();
        assert!(old.iter().any(|c| c.name == "旧目录"), "迁移不能丢老目录");
        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 预览图：统计、清空（返回待删路径）、只补缺图
    #[test]
    fn sheet_counts_and_clear_round_trip() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("K:/b.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/c.mp4", 0)).unwrap();
        let ids: Vec<i64> = {
            let mut stmt = ctx.conn.prepare("SELECT id FROM videos ORDER BY id").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
            rows.filter_map(Result::ok).collect()
        };
        set_contact_sheet(&ctx.conn, ids[0], "C:/tmp/sheet-1.jpg").unwrap();

        assert_eq!(sheet_counts(&ctx.conn).unwrap(), (2, 1, 1));
        assert_eq!(ids_without_sheet(&ctx.conn, 10).unwrap(), vec![ids[1]]);
        assert_eq!(all_video_ids_for_sheet(&ctx.conn, true).unwrap(), vec![ids[1]]);
        // 全量时缺图的排在前面
        assert_eq!(all_video_ids_for_sheet(&ctx.conn, false).unwrap().first(), Some(&ids[1]));

        let paths = clear_contact_sheets(&ctx.conn, None).unwrap();
        assert_eq!(paths, vec!["C:/tmp/sheet-1.jpg".to_string()]);
        assert_eq!(sheet_counts(&ctx.conn).unwrap(), (2, 0, 2));
        assert_eq!(ids_without_sheet(&ctx.conn, 10).unwrap().len(), 2);

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 自选封面：登记优先于接触表展示，撤销后回到空（接触表/中间帧兜底由上层决定）
    #[test]
    fn cover_set_clear_and_priority_in_query() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("K:/c.mp4", 0)).unwrap();
        let id: i64 = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get(0)).unwrap();

        // 初始：无自选封面
        assert_eq!(get_cover_path(&ctx.conn, id).unwrap(), None);
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.cover, "");

        // 重复设置 = 覆盖，查询端直接带到 VideoItem
        set_cover(&ctx.conn, id, "K:/covers/a.jpg").unwrap();
        set_cover(&ctx.conn, id, "K:/covers/b.jpg").unwrap();
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.cover, "K:/covers/b.jpg");
        assert_eq!(get_cover_path(&ctx.conn, id).unwrap().as_deref(), Some("K:/covers/b.jpg"));

        // 撤销后回到空
        clear_cover(&ctx.conn, id).unwrap();
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.cover, "");
        assert_eq!(get_cover_path(&ctx.conn, id).unwrap(), None);

        // contact_sheet 与 cover 互不干扰（预览图统计不受自选封面影响）
        set_contact_sheet(&ctx.conn, id, "K:/sheets/c.jpg").unwrap();
        set_cover(&ctx.conn, id, "K:/covers/c.jpg").unwrap();
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.contact_sheet, "K:/sheets/c.jpg");
        assert_eq!(item.cover, "K:/covers/c.jpg");
        assert_eq!(sheet_counts(&ctx.conn).unwrap(), (1, 1, 0));

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// v3 老库打开时补建 video_covers 表，查询端（带 cover 联查）照常工作
    #[test]
    fn v3_library_gains_covers_table_on_open() {
        let root = std::env::temp_dir().join(format!(
            "vv-mig4-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let vault = LibraryContext::vault_dir(&root);
        fs::create_dir_all(&vault).unwrap();
        {
            let conn = Connection::open(vault.join("vault.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE videos (
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
                   UNIQUE(path, entry_index));
                 INSERT INTO videos (path, file_name) VALUES ('K:/old.mp4', 'old.mp4');
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        }
        let ctx = open_library(&root).expect("v3 老库应能迁移");
        let version: i64 = ctx.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let item = get_video(&ctx.conn, 1).unwrap();
        assert_eq!(item.file_name, "old.mp4");
        assert_eq!(item.cover, "");
        set_cover(&ctx.conn, 1, "K:/covers/old.jpg").unwrap();
        assert_eq!(get_video(&ctx.conn, 1).unwrap().cover, "K:/covers/old.jpg");

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// v4 老库打开：collection_videos 补 position 列；红心收藏一次性迁入「我的收藏」；
    /// 已有同名夹（哪怕软删）绝不被复活或塞入
    #[test]
    fn v4_library_gains_position_and_migrates_favorites() {
        let root = std::env::temp_dir().join(format!(
            "vv-mig5-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let vault = LibraryContext::vault_dir(&root);
        fs::create_dir_all(&vault).unwrap();
        {
            let conn = Connection::open(vault.join("vault.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE videos (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   path TEXT NOT NULL, file_name TEXT NOT NULL, container TEXT NOT NULL DEFAULT '',
                   size_bytes INTEGER NOT NULL DEFAULT 0, mtime INTEGER NOT NULL DEFAULT 0,
                   duration_sec REAL NOT NULL DEFAULT 0, width INTEGER NOT NULL DEFAULT 0,
                   height INTEGER NOT NULL DEFAULT 0, codec_video TEXT NOT NULL DEFAULT '',
                   codec_audio TEXT NOT NULL DEFAULT '', entry_index INTEGER NOT NULL DEFAULT 0,
                   entry_count INTEGER NOT NULL DEFAULT 1, added_at INTEGER NOT NULL DEFAULT 0,
                   last_watched_at INTEGER NOT NULL DEFAULT 0, watch_count INTEGER NOT NULL DEFAULT 0,
                   progress_sec REAL NOT NULL DEFAULT 0, completed INTEGER NOT NULL DEFAULT 0,
                   rating INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '',
                   favorite INTEGER NOT NULL DEFAULT 0, trashed INTEGER NOT NULL DEFAULT 0,
                   contact_sheet TEXT NOT NULL DEFAULT '', tag_text TEXT NOT NULL DEFAULT '',
                   UNIQUE(path, entry_index));
                 INSERT INTO videos (path, file_name, favorite) VALUES ('K:/a.mp4','a.mp4',1),('K:/b.mp4','b.mp4',0),('K:/c.mp4','c.mp4',1);
                 CREATE TABLE collections (
                   id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'custom',
                   created_at INTEGER NOT NULL DEFAULT 0, UNIQUE(name, kind));
                 INSERT INTO collections (name, kind) VALUES ('待看','system'),('已有夹','custom');
                 CREATE TABLE collection_videos (
                   collection_id INTEGER NOT NULL, video_id INTEGER NOT NULL,
                   added_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (collection_id, video_id));
                 PRAGMA user_version = 4;",
            )
            .unwrap();
        }
        let ctx = open_library(&root).expect("v4 老库应能迁移");
        let version: i64 = ctx.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // 红心两条应已进入「我的收藏」，且带顺序位次；非红心的 b 不在
        let fav_id: i64 = ctx
            .conn
            .query_row("SELECT id FROM collections WHERE name='我的收藏'", [], |r| r.get(0))
            .unwrap();
        let members: Vec<(i64, i64)> = ctx
            .conn
            .prepare("SELECT video_id, position FROM collection_videos WHERE collection_id=?1 ORDER BY position")
            .unwrap()
            .query_map(params![fav_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(members.len(), 2, "favorite=1 的两条应迁入");
        assert_eq!(members[0].1, 0, "迁移位次从 0 开始");
        let names: Vec<String> = members
            .iter()
            .map(|(vid, _)| {
                ctx.conn
                    .query_row("SELECT file_name FROM videos WHERE id=?1", params![vid], |r| r.get(0))
                    .unwrap()
            })
            .collect();
        assert_eq!(names, vec!["a.mp4", "c.mp4"], "按 id 顺序迁移");

        // 再开一次库：不重复迁移（INSERT OR IGNORE 幂等）
        drop(ctx);
        let ctx2 = open_library(&root).unwrap();
        let n: i64 = ctx2
            .conn
            .query_row("SELECT COUNT(*) FROM collection_videos WHERE collection_id=?1", params![fav_id], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "重复打开不得再次迁移");

        drop(ctx2);
        let _ = fs::remove_dir_all(&root);
    }

    /// 改名/移动后只动这一条：路径、文件名、mtime 跟着走；auto 镜像夹成员搬到新夹，旧夹空了消失。
    #[test]
    fn apply_path_change_relinks_auto_folder() {
        let (root, ctx) = temp_lib();
        // 起始状态：文件在库根（没有文件夹），像「刚导入还没扫描」那种
        let src = root.join("a.mp4");
        fs::write(&src, b"x").unwrap();
        let mut r = rec(&src.to_string_lossy(), 0);
        r.file_name = "a.mp4".into();
        upsert_video(&ctx.conn, &r).unwrap();
        let id: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path=?1", params![src.to_string_lossy().to_string()], |x| x.get(0))
            .unwrap();

        let folder_of = |id: i64| -> String {
            ctx.conn
                .query_row(
                    "SELECT c.name FROM collections c JOIN collection_videos cv ON cv.collection_id=c.id WHERE cv.video_id=?1",
                    params![id],
                    |x| x.get(0),
                )
                .unwrap_or_default()
        };

        // 移动到 0-199：建夹 + 入成员
        fs::create_dir_all(root.join("0-199")).unwrap();
        let dst0 = root.join("0-199").join("a.mp4");
        fs::rename(&src, &dst0).unwrap();
        apply_path_change(&ctx.conn, &root, id, &dst0).unwrap();
        assert_eq!(folder_of(id), "0-199");

        // 移到 200-399：成员跟着搬，旧夹空了自动消失
        fs::create_dir_all(root.join("200-399")).unwrap();
        let dst = root.join("200-399").join("a.mp4");
        fs::rename(&dst0, &dst).unwrap();
        apply_path_change(&ctx.conn, &root, id, &dst).unwrap();
        assert_eq!(folder_of(id), "200-399");
        let old_left: i64 = ctx
            .conn
            .query_row("SELECT COUNT(*) FROM collections WHERE name='0-199' AND kind='auto'", [], |x| x.get(0))
            .unwrap();
        assert_eq!(old_left, 0, "空掉的镜像夹应删除");

        // 同目录改名：文件名/路径更新，所在夹不变
        let dst2 = root.join("200-399").join("b.mp4");
        fs::rename(&dst, &dst2).unwrap();
        apply_path_change(&ctx.conn, &root, id, &dst2).unwrap();
        let (path, name): (String, String) = ctx
            .conn
            .query_row("SELECT path, file_name FROM videos WHERE id=?1", params![id], |x| Ok((x.get(0)?, x.get(1)?)))
            .unwrap();
        assert!(path.ends_with("b.mp4"));
        assert_eq!(name, "b.mp4");
        assert_eq!(folder_of(id), "200-399");

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 排序方向：无前缀 = 各键既有默认方向（mtime/added 新近在前、title 升序），"-" 前缀取反。
    /// 这条锁住「只加逆序开关、不悄悄改老行为」。
    #[test]
    fn sort_keys_and_reverse_direction() {
        let (root, ctx) = temp_lib();
        let mut a = rec("K:/a.mp4", 0);
        a.file_name = "a.mp4".into();
        a.mtime = 300;
        let mut b = rec("K:/b.mp4", 0);
        b.file_name = "b.mp4".into();
        b.mtime = 100;
        let mut c = rec("K:/c.mp4", 0);
        c.file_name = "c.mp4".into();
        c.mtime = 200;
        upsert_video(&ctx.conn, &a).unwrap();
        upsert_video(&ctx.conn, &b).unwrap();
        upsert_video(&ctx.conn, &c).unwrap();

        let names = |sort: &str| -> Vec<String> {
            query_videos(&ctx.conn, &VideoQuery { sort: Some(sort.into()), ..Default::default() })
                .unwrap()
                .items
                .iter()
                .map(|v| v.file_name.clone())
                .collect()
        };
        assert_eq!(names("mtime"), vec!["a.mp4", "c.mp4", "b.mp4"], "mtime 默认新近在前");
        assert_eq!(names("-mtime"), vec!["b.mp4", "c.mp4", "a.mp4"], "-mtime 取反");
        assert_eq!(names("title"), vec!["a.mp4", "b.mp4", "c.mp4"], "title 默认升序");
        assert_eq!(names("-title"), vec!["c.mp4", "b.mp4", "a.mp4"], "-title 降序");
        // added 三条同秒入库时次序不稳，只验证两个方向都能查通
        assert_eq!(names("added").len(), 3);
        assert_eq!(names("-added").len(), 3);

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 夹内拖拽排序：子集重排 + 未入子集成员整体后移保序；manual 查询按位次返回
    #[test]
    fn reorder_collection_videos_normalizes_positions() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("K:/1.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/2.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/3.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/4.mp4", 0)).unwrap();
        let ids: Vec<i64> = (1..=4)
            .map(|i| {
                ctx.conn
                    .query_row("SELECT id FROM videos WHERE path=?1", params![format!("K:/{i}.mp4")], |r| r.get(0))
                    .unwrap()
            })
            .collect();
        let coll = create_collection(&ctx.conn, "片单", "").unwrap();
        for vid in &ids {
            set_collection_video(&ctx.conn, coll, *vid, true).unwrap();
        }

        // 初始位次 = 加入顺序 0..3；把 [3号, 1号] 拖到最前
        reorder_collection_videos(&ctx.conn, coll, &[ids[2], ids[0]]).unwrap();
        let order: Vec<i64> = ctx
            .conn
            .prepare(
                "SELECT cv.video_id FROM collection_videos cv WHERE cv.collection_id=?1 ORDER BY cv.position",
            )
            .unwrap()
            .query_map(params![coll], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(order, vec![ids[2], ids[0], ids[1], ids[3]], "子集置顶，未入子集的 2、4 号保序后移");

        // manual 查询按位次返回
        let coll_name = "片单";
        let page = query_videos(
            &ctx.conn,
            &VideoQuery { collection: Some(coll_name.into()), sort: Some("manual".into()), ..Default::default() },
        )
        .unwrap();
        let got: Vec<i64> = page.items.iter().map(|v| v.id).collect();
        assert_eq!(got, vec![ids[2], ids[0], ids[1], ids[3]]);

        // 新追加成员排到夹尾（位次 = MAX+1）
        upsert_video(&ctx.conn, &rec("K:/5.mp4", 0)).unwrap();
        let vid5: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path='K:/5.mp4'", [], |r| r.get(0))
            .unwrap();
        set_collection_video(&ctx.conn, coll, vid5, true).unwrap();
        let pos5: i64 = ctx
            .conn
            .query_row(
                "SELECT position FROM collection_videos WHERE collection_id=?1 AND video_id=?2",
                params![coll, vid5],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pos5, 6, "位次稀疏（子集重排后 2/4 号后移到 3/5），新成员 MAX+1=6 排到末尾");

        // 侧栏收藏夹顺序
        reorder_collections(&ctx.conn, &[coll, 1]).unwrap();
        let so: (i64, i64) = (
            ctx.conn.query_row("SELECT sort_order FROM collections WHERE id=?1", params![coll], |r| r.get(0)).unwrap(),
            ctx.conn.query_row("SELECT sort_order FROM collections WHERE id=1", [], |r| r.get(0)).unwrap(),
        );
        assert_eq!(so, (0, 1));

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 文件夹镜像：嵌套子目录 → auto 收藏夹（名=相对路径 › 连接，分组=首段）；
    /// 成员按目录归位、移动后重链、根目录直放不归类、手选夹不受影响
    #[test]
    fn sync_folder_collections_mirrors_disk_tree() {
        let (root, ctx) = temp_lib();
        for d in ["A", "A/B"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let paths = vec![
            root.join("A").join("a.mp4"),
            root.join("A").join("B").join("b.mp4"),
            root.join("A").join("合集.zip"),
            root.join("root.mp4"),
        ];
        for p in &paths {
            fs::write(p, b"x").unwrap();
        }
        let ps = |p: &Path| p.to_string_lossy().to_string();
        upsert_video(&ctx.conn, &rec(&ps(&paths[0]), 0)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[1]), 0)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[2]), 0)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[2]), 1)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[3]), 0)).unwrap();
        let manual = create_collection(&ctx.conn, "手选", "").unwrap();
        let root_vid: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path=?1", params![ps(&paths[3])], |r| r.get(0))
            .unwrap();
        set_collection_video(&ctx.conn, manual, root_vid, true).unwrap();

        let seen: Vec<String> = paths.iter().map(|p| ps(p)).collect();
        let synced = sync_folder_collections(&ctx.conn, &root, &seen).unwrap();
        assert_eq!(synced, 2, "A 与 A/B 两个文件夹镜像");

        let auto_of = |name: &str| -> Option<i64> {
            ctx.conn
                .query_row("SELECT id FROM collections WHERE name=?1 AND kind='auto'", params![name], |r| r.get(0))
                .optional()
                .unwrap()
        };
        let col_a = auto_of("A").expect("应有 auto 夹 A");
        let col_ab = auto_of("A › B").expect("应有 auto 夹 A › B");
        let group_of = |id: i64| -> String {
            ctx.conn
                .query_row("SELECT group_name FROM collections WHERE id=?1", params![id], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(group_of(col_a), "A", "分组 = 首段目录名");
        assert_eq!(group_of(col_ab), "A");

        let member_ids = |cid: i64| -> Vec<i64> {
            ctx.conn
                .prepare("SELECT video_id FROM collection_videos WHERE collection_id=?1 ORDER BY position")
                .unwrap()
                .query_map(params![cid], |r| r.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        let vid = |p: &Path| -> i64 {
            ctx.conn
                .query_row("SELECT id FROM videos WHERE path=?1", params![ps(p)], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            member_ids(col_a),
            vec![vid(&paths[0]), vid(&paths[2]), vid(&paths[2]) + 1],
            "A 夹：a.mp4 + 压缩包两集（入库顺序即位次）"
        );
        assert_eq!(member_ids(col_ab), vec![vid(&paths[1])], "A › B 夹：b.mp4");
        assert!(
            !member_ids(col_a).contains(&root_vid) && !member_ids(col_ab).contains(&root_vid),
            "根目录直放的文件不归类"
        );
        assert_eq!(member_ids(manual).len(), 1, "手选夹不受镜像影响");

        // 文件从 A/B 移到 A：重扫后链接应跟着走（旧位置清掉、新位置补链）
        // 注意：路径 UPDATE 后旧路径在 videos 里已不存在，断言一律用移动前捕获的 id
        let id_b = vid(&paths[1]);
        let moved = root.join("A").join("b.mp4");
        fs::rename(root.join("A").join("B").join("b.mp4"), &moved).unwrap();
        let n = ctx
            .conn
            .execute("UPDATE videos SET path=?2 WHERE path=?1", params![ps(&paths[1]), ps(&moved)])
            .unwrap();
        assert_eq!(n, 1);
        let seen2 = vec![ps(&paths[0]), ps(&moved), ps(&paths[2]), ps(&paths[3])];
        sync_folder_collections(&ctx.conn, &root, &seen2).unwrap();
        assert!(auto_of("A › B").is_none(), "B 目录已无视频，镜像夹应整夹消失");
        assert!(!member_ids(col_ab).contains(&id_b), "旧位置链接应被清掉");
        assert!(member_ids(col_a).contains(&id_b), "新位置应补链");

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 层级浏览：顶层=单段名夹；子层=前缀匹配；封面只取已有缓存，现场不生成
    #[test]
    fn list_folder_children_returns_layers() {
        let (root, ctx) = temp_lib();
        fs::create_dir_all(root.join("A")).unwrap();
        fs::create_dir_all(root.join("A/B")).unwrap();
        let paths = vec![
            root.join("A").join("a.mp4"),
            root.join("A").join("a2.mp4"),
            root.join("A").join("B").join("b.mp4"),
        ];
        for p in &paths {
            fs::write(p, b"x").unwrap();
        }
        let ps = |p: &Path| p.to_string_lossy().to_string();
        for p in &paths {
            upsert_video(&ctx.conn, &rec(&ps(p), 0)).unwrap();
        }
        let seen: Vec<String> = paths.iter().map(|p| ps(p)).collect();
        sync_folder_collections(&ctx.conn, &root, &seen).unwrap();

        // 顶层：只有 A（单段名），B 在下一层；a/a2 两条成员
        let top = list_folder_children(&ctx.conn, &root, None).unwrap();
        assert_eq!(top.len(), 1, "顶层应只有 A（根目录无视频不生成夹）");
        assert_eq!(top[0].leaf, "A");
        assert_eq!(top[0].name, "A");
        assert_eq!(top[0].count, 2);

        // 子层：A 下有 B；leaf 只剩末段
        let sub = list_folder_children(&ctx.conn, &root, Some("A")).unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "A › B");
        assert_eq!(sub[0].leaf, "B");
        assert_eq!(sub[0].count, 1);

        // 封面：无任何缓存 → 空 covers（现场不生成，占位由前端画）
        assert_eq!(top[0].covers.len(), 0, "没有缓存图时 covers 应为空");

        // 造一个成员的中帧缓存后重查 → covers 出现该路径
        let mid = LibraryContext::vault_dir(&root)
            .join("cache")
            .join("cover")
            .join("mid-1.jpg");
        fs::create_dir_all(mid.parent().unwrap()).unwrap();
        fs::write(&mid, b"fake-jpeg").unwrap();
        let top2 = list_folder_children(&ctx.conn, &root, None).unwrap();
        assert_eq!(top2[0].covers.len(), 1, "有缓存图的成员才进拼贴素材");
        assert!(top2[0].covers[0].ends_with("mid-1.jpg"));

        // 空层：B 下没有子文件夹
        let deeper = list_folder_children(&ctx.conn, &root, Some("A › B")).unwrap();
        assert_eq!(deeper.len(), 0);

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }
}
