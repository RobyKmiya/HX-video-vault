export interface VideoItem {
  id: number;
  path: string;
  file_name: string;
  container: string;
  size_bytes: number;
  mtime: number;
  duration_sec: number;
  width: number;
  height: number;
  codec_video: string;
  codec_audio: string;
  entry_index: number;
  entry_count: number;
  added_at: number;
  last_watched_at: number;
  watch_count: number;
  progress_sec: number;
  completed: boolean;
  rating: number;
  note: string;
  favorite: boolean;
  trashed: boolean;
  /** 导入计数：第几个被导入的视频（全局递增） */
  import_seq: number;
  /** 网页收藏带过来的出处（空 = 本地文件） */
  source_url: string;
  source_author: string;
  source_site: string;
  /** 位置计数：第几号文件夹 / 夹内第几条 */
  folder_no: number;
  slot_no: number;
  contact_sheet: string;
  /** 用户自选封面路径（优先于接触表展示；空 = 未自选） */
  cover: string;
  tag_text: string;
  tags: string[];
  collections: string[];
}

export interface VideoQuery {
  search?: string;
  tag?: string;
  collection?: string;
  view?: string;
  sort?: string;
  page?: number;
  page_size?: number;
}

export interface VideoPatch {
  id: number;
  rating?: number;
  note?: string;
  favorite?: boolean;
  completed?: boolean;
  progress_sec?: number;
}

export interface WatchUpdate {
  id: number;
  progress_sec: number;
  duration_sec: number;
  completed?: boolean;
}

export interface VideoPage {
  items: VideoItem[];
  total: number;
  page: number;
  page_size: number;
}

export interface Tag {
  id: number;
  name: string;
  category: string;
  weight: number;
  count: number;
}

export interface Collection {
  id: number;
  name: string;
  kind: string;
  count: number;
  group_name: string;
  deleted_at: number;
}

export interface DayCount {
  day: string;
  count: number;
}

export interface Stats {
  video_count: number;
  archive_count: number;
  total_bytes: number;
  total_duration_sec: number;
  watched_count: number;
  completed_count: number;
  favorite_count: number;
  tag_count: number;
  recent_days: DayCount[];
  top_tags: Tag[];
}

export interface ScanReport {
  found: number;
  added: number;
  updated: number;
  missing: number;
  skipped: number;
  /** 本次真正调用 ffprobe 的条目数 */
  probed?: number;
  /** 因为文件没变而跳过的条目数 */
  reused?: number;
  /** 文件夹镜像：本次同步的自动收藏夹数 */
  folders?: number;
  errors: string[];
}

/** 扫描进度事件（后端 emit "scan-progress"） */
export interface ScanProgress {
  phase: "walk" | "probe" | "write" | "done";
  done: number;
  total: number;
  added?: number;
  updated?: number;
}

export interface LibraryInfo {
  root: string;
  video_count: number;
  schema_version: number;
}

export interface Settings {
  library_root: string | null;
  ffmpeg_path: string | null;
  ffprobe_path: string | null;
  player_pref: string;
  mpv_path: string | null;
  /** 拖拽导入根目录；空 = 库根下的 Supervideo（区间夹 0-199 / 200-399…，每夹 200 个） */
  import_root?: string | null;
  /** 画廊双击行为：play = 直接播放（默认），detail = 打开详情 */
  double_click?: string;
  /** 播放时默认进入无边框沉浸模式 */
  immersive_default?: boolean;
  /** 卡片下方是否显示文件名与时长/大小 */
  show_card_info?: boolean;
  /** 影片墙卡片缩放倍率：0.6~1.8，1.0 = 默认 208px 宽。Ctrl+滚轮也能调 */
  grid_zoom?: number;
  /** 主题：dark（橙黑暗色，默认）/ light */
  theme?: string;
  /** 自定义主色（#rrggbb），空 = 用主题默认 */
  accent?: string;
}

/** 移动对话框的目标文件夹（库根 + 各 auto 文件夹） */
export interface MoveTarget {
  name: string;
  path: string;
  count: number;
}

/** 拖拽导入队列现场（后端 import-progress 事件 / import_status 命令） */
export interface ImportStatus {
  running: boolean;
  queue_total: number;
  queue_done: number;
  current_file: string;
  current_done: number;
  current_total: number;
  failed: number;
  last_error: string;
}

export interface DownloadTask {
  id: number;
  url: string;
  title: string;
  site: string;
  state: string;
  progress: number;
  target_path: string;
  tags: string;
  created_at: number;
  updated_at: number;
  error: string;
  bytes_done: number;
  bytes_total: number;
}

export interface MediaCandidate {
  url: string;
  name: string;
  kind: string;
}

export interface ResolveReport {
  site: string;
  title: string;
  candidates: MediaCandidate[];
  note: string;
  author?: string;
  tags?: string[];
}

export interface EngineInfo {
  name: string;
  available: boolean;
  path: string;
  note: string;
}

export interface WatchRecord {
  id: number;
  video_id: number;
  file_name: string;
  watched_at: number;
  from_sec: number;
  to_sec: number;
  duration_sec: number;
  finished: boolean;
}

export interface SheetReport {
  total: number;
  done: number;
  skipped: number;
  failed: string[];
  cancelled: boolean;
}

/** 预览图任务现场 */
export interface SheetStatus {
  running: boolean;
  total: number;
  done: number;
  pending: number;
}

/** 预览图进度事件 */
export interface SheetProgress {
  done: number;
  total: number;
  id: number;
  name?: string;
  finished?: boolean;
  cancelled?: boolean;
}

/** 剪辑素材：库内条目用 id，外来文件用 path */
export interface EditSource {
  id?: number;
  path?: string;
}

/** ffprobe 探测结果（外来素材加入剪辑面板时预览用） */
export interface MediaMeta {
  duration_sec: number;
  width: number;
  height: number;
  codec_video: string;
  codec_audio: string;
  fps: number;
}

/** 剪辑任务现场（后端 emit "edit-progress" 与 edit_status 返回同构） */
export interface EditStatus {
  running: boolean;
  /** idle | probe | run | done */
  phase: string;
  percent: number;
  message: string;
}

/** 主页层级浏览：一个子文件夹卡（auto 收藏夹 = 磁盘目录镜像） */
export interface FolderInfo {
  id: number;
  /** 完整路径名（A › B） */
  name: string;
  /** 末段名，卡片标题用 */
  leaf: string;
  count: number;
  /** 夹内最新入库时间（秒），用于按最新入库排文件夹 */
  latest_added: number;
  /** 封面拼贴素材（≤4 张已有图片路径） */
  covers: string[];
}

/** 剪辑结果 */
export interface EditReport {
  output_path: string;
  /** copy = 流复制；encode = 重新编码 */
  mode: string;
  note: string;
}

export interface PlaybackInfo {
  video: VideoItem;
  source: string;
  media_path: string;
  start_sec: number;
  player_hint: string;
}

/** 查重：同一内容的一个副本 */
export interface DuplicateItem {
  id: number;
  path: string;
  file_name: string;
  size_bytes: number;
  watch_count: number;
  last_watched_at: number;
  added_at: number;
  /** 文件此刻是否还在盘上 */
  exists: boolean;
}

/** 查重：一组内容重复 videos（同一 fingerprint） */
export interface DuplicateGroup {
  fingerprint: string;
  size_bytes: number;
  /** 建议保留的副本 */
  keep: DuplicateItem;
  others: DuplicateItem[];
  /** 清掉 others 能省下的字节 */
  wasted_bytes: number;
}

export interface FingerprintProgress {
  done: number;
  total: number;
  running: boolean;
}

/** 彻底删除（连磁盘文件）的执行结果 */
export interface PurgeReport {
  deleted: number;
  bytes: number;
  failed: string[];
}

/** 库整理结果 */
export interface OrganizeReport {
  moved: number;
  folders: number;
  skipped: string[];
}

/** 壁纸导出助手状态（Rust we_export::Status 序列化而来） */
export interface WeExportStatus {
  alive: boolean;
  pid: number | null;
  port: number;
  node_path: string;
  script: string;
  vault_root: string;
  config: {
    vaultLibrary?: string;
    steamLibrary?: string;
    weRoot?: string;
    library?: string;
    projectsRoot?: string;
    vaultctl?: string;
    autoScan?: boolean;
    includePreview?: boolean;
    nameFrom?: string;
    node?: string;
  };
  /** 助手 /status：导出进度 + 成功失败计数 + 最近结果 */
  export: {
    ok?: boolean;
    export?: {
      running: boolean;
      phase: string;
      total: number;
      done: number;
      ok: number;
      failed: number;
      skipped: number;
      current: string;
      message: string;
      startedAt: number;
      finishedAt: number;
    };
    history?: WeExportHistoryItem[];
  };
  logs: string[];
}

export interface WeExportHistoryItem {
  at: number;
  exported: number;
  failed: number;
  skipped: number;
  target: string;
}
