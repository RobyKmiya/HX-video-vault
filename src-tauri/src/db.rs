use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::models::{
    Collection, DayCount, DownloadTask, DuplicateGroup, DuplicateItem, LibraryInfo, Stats, Tag,
    VideoItem, VideoPage, VideoPatch, VideoQuery, WatchRecord, WatchUpdate,
};


// ── 共享类型 / 常量 / helper（被多个领域引用，留在此处）──
pub const SCHEMA_VERSION: i64 = 9;

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

const VIDEO_COLUMNS: &str = "id, path, file_name, container, size_bytes, mtime, duration_sec, width, height, codec_video, codec_audio, entry_index, entry_count, added_at, last_watched_at, watch_count, progress_sec, completed, rating, note, favorite, trashed, contact_sheet, import_seq, folder_no, slot_no, source_url, source_author, source_site, tag_text, (SELECT path FROM video_covers vc WHERE vc.video_id = v.id) AS cover, (SELECT GROUP_CONCAT(c.name, char(31)) FROM collection_videos cv JOIN collections c ON c.id=cv.collection_id WHERE cv.video_id=v.id AND c.deleted_at=0) AS collections";

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 本地时间（UTC+8）文件名友好格式，避免为一个格式化函数引入 chrono。
pub(crate) fn file_stamp() -> String {
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

pub(crate) fn map_video(row: &Row<'_>) -> rusqlite::Result<VideoItem> {
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
        import_seq: row.get("import_seq")?,
        folder_no: row.get("folder_no")?,
        slot_no: row.get("slot_no")?,
        source_url: row.get("source_url")?,
        source_author: row.get("source_author")?,
        source_site: row.get("source_site")?,
        // LEFT JOIN 没命中时是 NULL，走 Option 再落成空串
        cover: row.get::<_, Option<String>>("cover")?.unwrap_or_default(),
        tag_text,
        tags,
        collections,
    })
}

/// 路径比较用的规范化键：Windows 上盘符大小写和 / 与 \ 混用都指向同一个文件，
/// 历史版本按原始字符串登记，导致同一部影片出现多条记录。
pub(crate) fn path_key(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
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

pub(crate) fn dup_item_from_row(r: &Row) -> rusqlite::Result<DuplicateItem> {
    let path: String = r.get(1)?;
    let exists = Path::new(&path).is_file();
    Ok(DuplicateItem {
        id: r.get(0)?,
        path,
        file_name: r.get(2)?,
        size_bytes: r.get(3)?,
        watch_count: r.get(4)?,
        last_watched_at: r.get(5)?,
        added_at: r.get(6)?,
        exists,
    })
}

/// 最近加入：批量导入的 added_at 是秒级，常常整批相同。此时优先按 id 倒序
/// （id 是递增的入库次序），再按 mtime 兜底；不能用 mtime 当第二键，否则
/// 导入时继承了源文件旧 mtime 的新片会被压到后面，看不到「最新」。
pub(crate) fn added_order(desc: bool) -> String {
    let d = if desc { "DESC" } else { "ASC" };
    format!("v.added_at {d}, v.id {d}, v.mtime {d}")
}

pub(crate) fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// 绝对路径 → 相对库根的文件夹名（"A › B"）；库根直放或不在库里返回 None。
pub(crate) fn rel_dir_of(root: &Path, path: &str) -> Option<String> {
    let root_str = root.to_string_lossy();
    let rel = path.strip_prefix(root_str.as_ref())?.trim_start_matches(['\\', '/']);
    let idx = rel.rfind(['\\', '/'])?;
    if idx == 0 {
        return None;
    }
    Some(rel[..idx].replace(['\\', '/'], " › "))
}

pub(crate) fn auto_collection_id(conn: &Connection, name: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT id FROM collections WHERE name=?1 AND kind='auto'",
        params![name],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// 取（必要时建）某个目录对应的 auto 镜像夹；被软删过的一并复活。
pub(crate) fn ensure_auto_collection(conn: &Connection, name: &str) -> Result<i64, String> {
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
pub(crate) fn prune_empty_auto(conn: &Connection, collection_id: i64) -> Result<(), String> {
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

const DOWNLOAD_COLUMNS: &str = "id, url, title, site, state, progress, target_path, tags, created_at, updated_at, error, bytes_done, bytes_total";

pub(crate) fn map_download(row: &Row<'_>) -> rusqlite::Result<DownloadTask> {
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

// ── 领域子模块：全部 pub 函数 re-export 回 crate::db::xxx，外部调用路径不变 ──
pub(crate) mod schema;
pub use schema::*;
pub(crate) mod dedupe;
pub use dedupe::*;
pub(crate) mod videos;
pub use videos::*;
pub(crate) mod covers;
pub use covers::*;
pub(crate) mod tags;
pub use tags::*;
pub(crate) mod collections;
pub use collections::*;
pub(crate) mod folder;
pub use folder::*;
pub(crate) mod stats;
pub use stats::*;
pub(crate) mod downloads;
pub use downloads::*;
pub(crate) mod organize;
pub use organize::*;
pub(crate) mod hooks;
pub use hooks::*;

#[cfg(test)]
mod tests;
