import { invoke } from "@tauri-apps/api/core";
import type {
  Collection,
  LibraryInfo,
  PlaybackInfo,
  ScanReport,
  Settings,
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
  collections: () => invoke<Collection[]>("list_collections"),
  createCollection: (name: string) => invoke<void>("create_collection", { name }),
  setCollection: (name: string, videoId: number, add: boolean) =>
    invoke<void>("set_collection_video", { name, videoId, add }),
  stats: () => invoke<Stats>("stats"),
  playback: (id: number) => invoke<PlaybackInfo>("playback_info", { id }),
  playExternal: (id: number) => invoke<string>("play_external", { id }),
  contactSheet: (id: number) => invoke<string>("make_contact_sheet", { id }),
};
