use serde::{Deserialize, Serialize};

/// 库里的一「条」视频。压缩包里的每个视频条目都是独立的一条，
/// 这样可以单独记进度、单独打标签（画匣是「一本 = 一个文件」，视频不能照搬）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoItem {
    pub id: i64,
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
    /// 在压缩包/合集内的序号（普通文件恒为 0）
    pub entry_index: i64,
    pub entry_count: i64,
    pub added_at: i64,
    pub last_watched_at: i64,
    pub watch_count: i64,
    /// 上次看到第几秒
    pub progress_sec: f64,
    pub completed: bool,
    pub rating: i64,
    pub note: String,
    pub favorite: bool,
    pub trashed: bool,
    /// 接触表相对路径（真空白表示尚未生成）
    pub contact_sheet: String,
    /// 冗余的「原文 + 译名」标签串，供搜索
    pub tag_text: String,
    pub tags: Vec<String>,
    /// 该视频所在的目录名（含系统目录「待看」）
    pub collections: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VideoQuery {
    pub search: Option<String>,
    pub tag: Option<String>,
    pub collection: Option<String>,
    pub view: Option<String>,
    pub sort: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoPage {
    pub items: Vec<VideoItem>,
    pub total: i64,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub weight: i64,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub video_count: i64,
    pub archive_count: i64,
    pub total_bytes: i64,
    pub total_duration_sec: f64,
    pub watched_count: i64,
    pub completed_count: i64,
    pub favorite_count: i64,
    pub tag_count: i64,
    pub recent_days: Vec<DayCount>,
    pub top_tags: Vec<Tag>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayCount {
    pub day: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub found: usize,
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanOptions {
    pub root: String,
    pub recursive: Option<bool>,
    /// 每个压缩包最多登记多少个视频条目（防止超大合集卡死 UI）
    pub entry_limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchUpdate {
    pub id: i64,
    pub progress_sec: f64,
    pub duration_sec: f64,
    pub completed: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoPatch {
    pub id: i64,
    pub rating: Option<i64>,
    pub note: Option<String>,
    pub favorite: Option<bool>,
    pub completed: Option<bool>,
    pub progress_sec: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryInfo {
    pub root: String,
    pub video_count: i64,
    pub schema_version: i64,
}
