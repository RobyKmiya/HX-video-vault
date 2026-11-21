import { invoke } from "@tauri-apps/api/core";
import type {
  Collection,
  DownloadTask,
  EditReport,
  EditSource,
  EditStatus,
  EngineInfo,
  LibraryInfo,
  MediaMeta,
  ResolveReport,
  PlaybackInfo,
  ScanReport,
  Settings,
  SheetReport,
  SheetStatus,
  WatchRecord,
  Stats,
  Tag,
  VideoItem,
  VideoPage,
  VideoPatch,
  VideoQuery,
  WatchUpdate,
} from "./types";

// 后端 ScanOptions 在 types 中无需重复声明，这里补齐
export interface ScanOptionsLocal {
  root: string;
  recursive?: boolean;
  entry_limit?: number;
}

type AppSettingsBundle = [Settings, string | null, LibraryInfo | null];

export const api = {
  settings: () => invoke<AppSettingsBundle>("app_settings"),
  setLibrary: (root: string) => invoke<LibraryInfo>("set_library", { root }),
  updateSettings: (settings: Settings) => invoke<void>("update_settings", { settings }),
  pickDirectory: () => invoke<string | null>("pick_directory"),
  scan: (opts: ScanOptionsLocal) => invoke<ScanReport>("scan", { opts }),
  query: (q: VideoQuery) => invoke<VideoPage>("query_videos", { q }),
  video: (id: number) => invoke<VideoItem>("get_video", { id }),
  patch: (patch: VideoPatch) => invoke<void>("patch_video", { patch }),
  watch: (u: WatchUpdate) => invoke<void>("record_watch", { u }),
  trash: (id: number, trashed: boolean) => invoke<void>("trash_video", { id, trashed }),
  tags: () => invoke<Tag[]>("list_tags"),
  createTag: (name: string, category: string, weight: number) =>
    invoke<number>("create_tag", { name, category, weight }),
  tagVideo: (videoId: number, tagId: number, add: boolean) =>
    invoke<void>("set_tag_video", { videoId, tagId, add }),
  updateTag: (id: number, name: string, category: string, weight: number) =>
    invoke<void>("update_tag", { id, name, category, weight }),
  bulkTag: (videoIds: number[], tagIds: number[], add: boolean) =>
    invoke<number>("bulk_tag", { videoIds, tagIds, add }),
  collections: (includeDeleted = false) => invoke<Collection[]>("list_collections", { includeDeleted }),
  createCollection: (name: string, group = "") => invoke<number>("create_collection", { name, group }),
  renameCollection: (id: number, name: string) => invoke<void>("rename_collection", { id, name }),
  setCollectionGroup: (id: number, group: string) => invoke<void>("set_collection_group", { id, group }),
  deleteCollection: (id: number, purge = false) => invoke<void>("delete_collection", { id, purge }),
  restoreCollection: (id: number) => invoke<void>("restore_collection", { id }),
  /** 目录 id 或名字都能用（详情页拿 id，旧调用点写名字） */
  setCollection: (target: number | string, videoId: number, add: boolean) =>
    typeof target === "number"
      ? invoke<void>("set_collection_video", { collectionId: target, videoId, add })
      : invoke<void>("set_collection_video", { name: target, videoId, add }),
  stats: () => invoke<Stats>("stats"),
  playback: (id: number) => invoke<PlaybackInfo>("playback_info", { id }),
  playExternal: (id: number) => invoke<string>("play_external", { id }),
  contactSheet: (id: number) => invoke<string>("make_contact_sheet", { id }),
  contactSheets: (ids: number[] | null, limit?: number, all = false) =>
    invoke<SheetReport>("make_contact_sheets", { ids, limit, all }),
  cancelContactSheets: () => invoke<void>("cancel_contact_sheets"),
  clearContactSheets: (ids?: number[] | null) => invoke<number>("clear_contact_sheets", { ids: ids ?? null }),
  sheetStatus: () => invoke<SheetStatus>("sheet_status"),
  watchHistory: (limit?: number) => invoke<WatchRecord[]>("watch_history", { limit }),
  engines: () => invoke<EngineInfo[]>("player_engines"),
  mpvOpen: (id: number, wid?: string) => invoke<boolean>("mpv_open", { id, wid }),
  mpvCommand: (action: string, value?: unknown) => invoke<void>("mpv_command", { action, value }),
  mpvClose: () => invoke<void>("mpv_close"),
  resolveLink: (url: string) => invoke<ResolveReport>("resolve_link", { url }),
  downloads: () => invoke<DownloadTask[]>("list_downloads"),
  addDownload: (url: string, title: string, site: string, targetPath: string, tags: string) =>
    invoke<number>("add_download", { url, title, site, targetPath, tags }),
  removeDownload: (id: number, deleteFile: boolean) =>
    invoke<void>("remove_download", { id, deleteFile }),
  startDownloads: () => invoke<number>("start_downloads"),
  trimVideo: (source: EditSource, startSec: number, endSec: number, accurate: boolean) =>
    invoke<EditReport>("trim_video", { source, startSec, endSec, accurate }),
  concatVideo: (sources: EditSource[]) => invoke<EditReport>("concat_video", { sources }),
  editStatus: () => invoke<EditStatus>("edit_status"),
  cancelEdit: () => invoke<void>("cancel_edit"),
  pickVideoFiles: () => invoke<string[]>("pick_video_files"),
  probeMedia: (path: string) => invoke<MediaMeta>("probe_media", { path }),
  /** 最佳封面路径：自选封面 > 中间帧兜底（懒生成）；卡片仅在两者都缺时调用 */
  videoCover: (id: number) => invoke<string>("video_cover", { id }),
  /** 内置播放器把 canvas 截下的当前帧（JPEG data URL）设为封面 */
  setCoverImage: (id: number, dataUrl: string) => invoke<void>("set_cover_image", { id, dataUrl }),
  /** mpv screenshot-to-file 应写的目标绝对路径 */
  mpvCoverPath: (id: number) => invoke<string>("mpv_cover_path", { id }),
  /** mpv 已把当前帧写到目标路径后登记为封面 */
  setCoverFromMpv: (id: number, path: string) => invoke<void>("set_cover_from_mpv", { id, path }),
  clearCover: (id: number) => invoke<void>("clear_cover", { id }),
};
