//! `schema` 数据库操作，从 db.rs 拆出；逻辑未改。
use super::organize::backfill_positions;
use super::*;

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
    migrate(&conn, root)?;
    daily_backup(root, &db_path);
    Ok(LibraryContext {
        root: root.to_path_buf(),
        db_path,
        conn,
    })
}
/// v6 清理：规范化路径分隔符、合并重复登记、删除不在当前库根下的历史残留。
/// 关联记录（标签/收藏/封面/观看历史）先改挂到保留的那条，再删重复视频行。
fn normalize_and_dedupe_videos(conn: &Connection, root: &Path) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String, i64, i64)> = {
        let mut stmt = tx
            .prepare("SELECT id, path, entry_index, trashed FROM videos ORDER BY trashed ASC, id ASC")
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        mapped.filter_map(Result::ok).collect()
    };

    // 同 (规范化路径, 条目序号) 只留最早/未回收的一条
    let mut keepers: HashMap<(String, i64), i64> = HashMap::new();
    let mut duplicates: Vec<(i64, i64)> = Vec::new();
    for (id, path, idx, _) in &rows {
        let key = (path_key(path), *idx);
        if let Some(keep) = keepers.get(&key) {
            duplicates.push((*id, *keep));
        } else {
            keepers.insert(key, *id);
        }
    }
    for (dup, keep) in &duplicates {
        for sql in [
            "INSERT OR IGNORE INTO video_tags(video_id, tag_id) SELECT ?1, tag_id FROM video_tags WHERE video_id=?2",
            "INSERT OR IGNORE INTO collection_videos(collection_id, video_id, added_at, position) SELECT collection_id, ?1, added_at, position FROM collection_videos WHERE video_id=?2",
            "INSERT OR IGNORE INTO video_covers(video_id, path, created_at) SELECT ?1, path, created_at FROM video_covers WHERE video_id=?2",
        ] {
            tx.execute(sql, params![keep, dup]).map_err(|e| e.to_string())?;
        }
        tx.execute("UPDATE watch_history SET video_id=?1 WHERE video_id=?2", params![keep, dup])
            .map_err(|e| e.to_string())?;
        for table in ["video_tags", "collection_videos", "video_covers"] {
            tx.execute(&format!("DELETE FROM {table} WHERE video_id=?1"), params![dup])
                .map_err(|e| e.to_string())?;
        }
        tx.execute("DELETE FROM videos WHERE id=?1", params![dup])
            .map_err(|e| e.to_string())?;
    }

    // 统一成正斜杠→反斜杠；大小写不敏感的重复已在上面合并，这里不会撞 UNIQUE
    for (id, path, _, _) in &rows {
        if duplicates.iter().any(|(dup, _)| dup == id) {
            continue;
        }
        let normalized = path.replace('/', "\\");
        if normalized != *path {
            tx.execute("UPDATE videos SET path=?1 WHERE id=?2", params![normalized, id])
                .map_err(|e| e.to_string())?;
        }
    }

    // 清掉库根之外的旧记录（历史上库根是 F:\Hthings，扫到了 B / BENZI / MOST 等）
    let root_prefix = format!("{}\\", path_key(&root.to_string_lossy()).trim_end_matches('\\'));
    let remaining: Vec<(i64, String)> = {
        let mut stmt = tx
            .prepare("SELECT id, path FROM videos")
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        mapped.filter_map(Result::ok).collect()
    };
    for (id, path) in &remaining {
        let key = path_key(path);
        if key == root_prefix.trim_end_matches('\\') || key.starts_with(&root_prefix) {
            continue;
        }
        // 外键级联会清掉关联记录
        tx.execute("DELETE FROM videos WHERE id=?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

// ── 内容查重 ──────────────────────────────────────────────────────────
// 路径去重（normalize_and_dedupe_videos）只认「同一条路径登记了两次」。
// 用户把同一个文件复制进两个区间夹时路径不同，那套逻辑看不见，库里的
// 真重复就是这么攒出来的（实测 39 组、23.2GB）。这里按内容指纹找。
fn migrate(conn: &Connection, root: &Path) -> Result<(), String> {
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
  -- 内容指纹（大小 + 首尾各 8MB 的 blake3），查重用；空 = 还没算过
  fingerprint TEXT NOT NULL DEFAULT '',
  -- 被判定为重复后指向保留的那条的 id；0 = 自己就是保留项
  dedup_of INTEGER NOT NULL DEFAULT 0,
  -- 导入计数：第几个被导入的视频（全局递增，按 added_at/id）
  import_seq INTEGER NOT NULL DEFAULT 0,
  -- 位置计数：第几号文件夹（1,2,3…）、夹内第几条（1..200）
  folder_no INTEGER NOT NULL DEFAULT 0,
  slot_no INTEGER NOT NULL DEFAULT 0,
  UNIQUE(path, entry_index)
);
CREATE INDEX IF NOT EXISTS idx_videos_path ON videos(path);
CREATE INDEX IF NOT EXISTS idx_videos_added ON videos(added_at DESC);
CREATE INDEX IF NOT EXISTS idx_videos_watched ON videos(last_watched_at DESC);
CREATE INDEX IF NOT EXISTS idx_videos_size ON videos(size_bytes DESC);
-- 注意：fingerprint 的索引不能放这里。老库的 videos 表已存在、可能还没有这一列，
-- 批次里建索引会直接报 no such column。放到 v7 迁移补完列之后再建。

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
    // v5 → v6：统一路径分隔符、按规范化路径合并重复登记，并清掉库根之外的历史残留
    if version < 6 {
        normalize_and_dedupe_videos(conn, root)?;
    }
    // v6 → v7：加内容指纹列，为「同一文件被复制到两个区间夹」这类重复提供判据
    if version < 7 {
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(videos)").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        for (name, ddl) in [
            ("fingerprint", "ALTER TABLE videos ADD COLUMN fingerprint TEXT NOT NULL DEFAULT ''"),
            ("dedup_of", "ALTER TABLE videos ADD COLUMN dedup_of INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !cols.iter().any(|c| c == name) {
                conn.execute_batch(ddl).map_err(|e| e.to_string())?;
            }
        }
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_videos_fp ON videos(fingerprint)")
            .map_err(|e| e.to_string())?;
    }
    // v7 → v8：加导入计数 / 位置计数。导入计数按入伙顺序递增；位置计数从现有目录名反推
    //（纯数字夹 = 第几号夹，`0-199` 这类区间夹按 200 一段折成 1..N），夹内序号按导入顺序。
    if version < 8 {
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(videos)").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        for (name, ddl) in [
            ("import_seq", "ALTER TABLE videos ADD COLUMN import_seq INTEGER NOT NULL DEFAULT 0"),
            ("folder_no", "ALTER TABLE videos ADD COLUMN folder_no INTEGER NOT NULL DEFAULT 0"),
            ("slot_no", "ALTER TABLE videos ADD COLUMN slot_no INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !cols.iter().any(|c| c == name) {
                conn.execute_batch(ddl).map_err(|e| e.to_string())?;
            }
        }
        // 导入计数：同一秒按 id 兜底，保证严格全序
        conn.execute_batch(
            "UPDATE videos SET import_seq = (
               SELECT COUNT(*) FROM videos v2
               WHERE v2.added_at < videos.added_at
                  OR (v2.added_at = videos.added_at AND v2.id <= videos.id)
             );",
        )
        .map_err(|e| e.to_string())?;
        backfill_positions(conn)?;
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_videos_import_seq ON videos(import_seq)")
            .map_err(|e| e.to_string())?;
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
