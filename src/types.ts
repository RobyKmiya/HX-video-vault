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
