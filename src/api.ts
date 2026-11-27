import { invoke } from "@tauri-apps/api/core";
import type {
  Collection,
  DownloadTask,
  DuplicateGroup,
  EditReport,
  EditSource,
  EditStatus,
  EngineInfo,
  FingerprintProgress,
  FolderInfo,
  ImportStatus,
  LibraryInfo,
  MediaMeta,
  MoveTarget,
  PlaybackInfo,
  PurgeReport,
  ResolveReport,
  ScanReport,
  Settings,
  SheetReport,
  SheetStatus,
  Stats,
  Tag,
  VideoItem,
  VideoPage,
  VideoPatch,
  VideoQuery,
  WatchRecord,
  WatchUpdate
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
  /** 彻底删除回收站条目：连磁盘文件一起删，不可恢复 */
  purgeVideos: (ids: number[]) => invoke<PurgeReport>("purge_videos", { ids }),
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
  /** 侧栏收藏夹拖拽排序：传拖完后的完整 id 顺序 */
  reorderCollections: (ids: number[]) => invoke<void>("reorder_collections", { ids }),
  /** 收藏夹内视频拖拽排序：传可见子集的新顺序，未入子集成员整体后移保序 */
  reorderCollectionVideos: (collectionId: number, videoIds: number[]) =>
    invoke<void>("reorder_collection_videos", { collectionId, videoIds }),
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
  /** 内置播放器把当前秒数交给后端 ffmpeg 精确抽帧当封面（canvas 在 asset 协议下被污染不可用） */
  setCoverFromTime: (id: number, sec: number) => invoke<void>("set_cover_from_time", { id, sec }),
  clearCover: (id: number) => invoke<void>("clear_cover", { id }),
  /** 主页层级浏览：某层下的 auto 子文件夹卡（parent=null 取顶层） */
  listFolderChildren: (parent: string | null) =>
    invoke<FolderInfo[]>("list_folder_children", { parent }),
  /** 拖拽导入：外部文件路径入队后台复制进区间夹，返回入队数 */
  importVideos: (paths: string[]) => invoke<number>("import_videos", { paths }),
  importStatus: () => invoke<ImportStatus>("import_status"),
  cancelImport: () => invoke<void>("cancel_import"),
  /** 文件夹视图手动刷新封面：只重置这个夹的缓存图（不全库慢扫），返回被重置的成员 id */
  resetFolderCovers: (collectionId: number) =>
    invoke<number[]>("reset_folder_covers", { collectionId }),
  /** 原地重命名（只改文件名，不动文件内容与封面缓存），返回更新后的整条 */
  renameVideo: (id: number, name: string) => invoke<VideoItem>("rename_video", { id, name }),
  /** 批量移动到库内某个绝对路径目录，返回成功条数 */
  moveVideos: (ids: number[], target: string) => invoke<number>("move_videos", { ids, target }),
  /** 移动目标：库根 + 所有 auto 文件夹 */
  moveTargets: () => invoke<MoveTarget[]>("move_targets"),
  /** 资源管理器里定位并选中该文件 */
  revealVideo: (id: number) => invoke<void>("reveal_video", { id }),
  /** 沉浸播放：只去掉系统标题栏（**不**强制全屏），最大化状态下会自动取消/恢复最大化 */
  setImmersive: (on: boolean) => invoke<void>("set_immersive", { on }),
  /** 真全屏（系统级窗口全屏，铺满屏幕）；与「无边框沉浸」是两回事 */
  setFullscreen: (on: boolean) => invoke<void>("set_fullscreen", { on }),
  /** 老板键第一步：把主窗整个藏起来；恢复靠系统级全局热键 */
  hideMainWindow: () => invoke<void>("hide_main_window"),
  /** 老板键呼回键（真值在 Rust 侧：注册过的才算数） */
  getBossRecall: () => invoke<string>("get_boss_recall"),
  /** 改呼回键：Rust 注册成功才落盘，失败会返回错误 */
  setBossRecall: (binding: string) => invoke<void>("set_boss_recall", { binding }),
  /** 把所选视频打包成 zip（保留原文件），返回压缩包路径 */
  compressVideos: (ids: number[], name?: string) =>
    invoke<string>("compress_videos", { ids, name: name ?? null }),
  /** 后台算内容指纹（查重第一步）。已在跑时直接返回 */
  dedupeScan: () => invoke<void>("dedupe_scan"),
  dedupeStatus: () => invoke<FingerprintProgress>("dedupe_status"),
  /** 算出指纹后才能拿到重复组 */
  dedupeGroups: () => invoke<DuplicateGroup[]>("dedupe_groups"),
  /** 按后端建议（看过次数多者留）把所有重复副本送进回收站，返回处理条数 */
  dedupeApply: () => invoke<number>("dedupe_apply"),
  /** 精确处理：[[保留 id, [丢弃 id...]], ...] */
  dedupeApplySelected: (pairs: [number, number[]][]) =>
    invoke<number>("dedupe_apply_selected", { pairs }),
  /** 清掉回收站里文件已不在的幽灵行 */
  purgeMissingTrashed: () => invoke<number>("purge_missing_trashed"),
};
