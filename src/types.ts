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
  errors: string[];
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
}

export interface PlaybackInfo {
  video: VideoItem;
  source: string;
  media_path: string;
  start_sec: number;
  player_hint: string;
}
