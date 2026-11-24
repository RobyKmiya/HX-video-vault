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
    /// 用户自选封面路径（优先于接触表展示；空 = 未自选，回退接触表/中间帧）
    pub cover: String,
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
    /// 自定义分组（空串 = 未分组）
    pub group_name: String,
    /// 软删除时间戳，0 = 正常
    pub deleted_at: i64,
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
    /// 本次真正调用 ffprobe 的条目数
    #[serde(default)]
    pub probed: usize,
    /// 因为文件没变而跳过的条目数
    #[serde(default)]
    pub reused: usize,
    /// 文件夹镜像：本次同步的自动收藏夹数（磁盘子目录 → kind='auto'）
    #[serde(default)]
    pub folders: usize,
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

#[derive(Debug, Clone, Serialize)]
pub struct WatchRecord {
    pub id: i64,
    pub video_id: i64,
    pub file_name: String,
    pub watched_at: i64,
    pub from_sec: f64,
    pub to_sec: f64,
    pub duration_sec: f64,
    pub finished: bool,
}

/// 下载任务（对应 download_tasks 表）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub site: String,
    pub state: String,        // queued | running | paused | done | failed | canceled
    pub progress: f64,        // 0..1
    pub target_path: String,
    pub tags: String,         // 逗号分隔，完成后自动挂到入库条目
    pub created_at: i64,
    pub updated_at: i64,
    pub error: String,
    pub bytes_done: i64,
    pub bytes_total: i64,
}

/// 站点解析结果：一个链接里发现的媒体候选
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaCandidate {
    pub url: String,
    pub name: String,
    pub kind: String, // video | image | archive
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolveReport {
    pub site: String,
    pub title: String,
    pub candidates: Vec<MediaCandidate>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SheetReport {
    pub total: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
    /// 被用户中途停掉（已生成的都已落库，下次接着跑）
    #[serde(default)]
    pub cancelled: bool,
}

/// 预览图任务现场（面板显示用）
#[derive(Debug, Clone, Serialize)]
pub struct SheetStatus {
    pub running: bool,
    pub total: i64,
    pub done: i64,
    pub pending: i64,
}

/// 剪辑素材：库内条目（按 id，压缩包条目自动解到缓存）或任意本地视频文件。
#[derive(Debug, Clone, Deserialize)]
pub struct EditSource {
    pub id: Option<i64>,
    pub path: Option<String>,
}

/// 主页层级浏览的一个子文件夹卡（auto 收藏夹 = 磁盘目录镜像）。
#[derive(Debug, Clone, Serialize)]
pub struct FolderInfo {
    pub id: i64,
    /// 完整路径名（A › B）
    pub name: String,
    /// 末段名（B），卡片标题用
    pub leaf: String,
    /// 夹内视频条数（压缩包条目各算一条）
    pub count: i64,
    /// 夹内最新入库时间（秒），用于「按最新入库排文件夹」
    pub latest_added: i64,
    /// 封面拼贴素材（≤4 张已有图片：自选封面 > 中帧缓存 > 接触表，存在哪个算哪个）
    pub covers: Vec<String>,
}

/// 剪辑任务现场（面板显示用）。percent 0..100，-1 表示无法估进度（仅拼接 copy 前探测期）。
#[derive(Debug, Clone, Serialize)]
pub struct EditStatus {
    pub running: bool,
    /// idle | probe | run | done
    pub phase: String,
    pub percent: f64,
    pub message: String,
}

/// 剪辑结果：成品路径 + 实际走的编码方式。
#[derive(Debug, Clone, Serialize)]
pub struct EditReport {
    pub output_path: String,
    /// copy = 流复制（秒出）；encode = 重新编码（慢）
    pub mode: String,
    pub note: String,
}
