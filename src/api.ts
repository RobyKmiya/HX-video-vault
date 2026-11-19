import { invoke } from "@tauri-apps/api/core";
import type {
  Collection,
  DownloadTask,
  EngineInfo,
  LibraryInfo,
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
};
