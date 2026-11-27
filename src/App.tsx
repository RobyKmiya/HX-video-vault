import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, DragEvent, MouseEvent as ReactMouseEvent } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Icon } from "./components/Icons";
import { Sidebar, type ViewKey } from "./components/Sidebar";
import { VideoCard } from "./components/VideoCard";
import { DetailDrawer } from "./components/DetailDrawer";
import { Player } from "./components/Player";
import { StatsPanel } from "./components/StatsPanel";
import { TagManager } from "./components/TagManager";
import { DownloadPanel } from "./components/DownloadPanel";
import { DedupePanel } from "./components/DedupePanel";
import { EditPanel } from "./components/EditPanel";
import { MoveDialog } from "./components/MoveDialog";
import { RenameDialog } from "./components/RenameDialog";
import { Splitter } from "./components/Splitter";
import { Toast, type ToastData, type ToastKind } from "./components/Toast";
import { api } from "./api";
import { prewarmCovers, resetCovers } from "./cover";
import { formatBytes } from "./format";
import { usePersistentSize } from "./layout";
import { rangeSelection, rectFromPoints, rectsIntersect } from "./selection";
import { loadGestures, type GestureMap } from "./gestures";
import { loadKeyMap, comboFromEvent, actionForCombo, type ActionId, type KeyMap } from "./shortcuts";
import type {
  Collection,
  FolderInfo,
  ImportStatus,
  LibraryInfo,
  PlaybackInfo,
  ScanProgress,
  Settings,
  SheetProgress,
  SheetStatus,
  Stats,
  Tag,
  VideoItem,
  VideoPage,
} from "./types";

import { BootSplash } from "./components/BootSplash";
import { ImportHud } from "./components/ImportHud";
import { SettingsModal } from "./components/SettingsModal";
import { SheetPanel } from "./components/SheetPanel";
import { CollectionManager } from "./components/CollectionManager";

/** 导航历史的一帧：主页结构导航（视图/文件夹/标签/收藏夹/排序+方向） */
type NavLoc = { view: ViewKey; folder: string[]; tag: string | null; coll: string | null; sort: string; rev: boolean };

const SORTS = [
  { key: "folder", label: "文件夹" },
  { key: "added", label: "最近加入" },
  { key: "import", label: "导入顺序" },
  { key: "mtime", label: "文件修改时间" },
  { key: "watched", label: "最近观看" },
  { key: "title", label: "文件名" },
  { key: "size", label: "大小" },
  { key: "duration", label: "时长" },
  { key: "count", label: "观看次数" },
  { key: "rating", label: "评分" },
  { key: "manual", label: "手动排序（收藏夹拖拽）" },
];

const SCAN_PHASE: Record<ScanProgress["phase"], string> = {
  walk: "清点",
  probe: "识别",
  write: "入库",
  done: "完成",
};

/** 影片墙卡片基准宽度 / 间距（1.0 倍率下的值，与 App.css 的兜底值一致） */
const GRID_CARD_BASE = 208;
const GRID_GAP_BASE = 16;
const GRID_ZOOM_MIN = 0.6;
const GRID_ZOOM_MAX = 1.8;
/** 快捷键与按钮每次调多少 */
const GRID_ZOOM_STEP = 0.1;

const EMPTY_PAGE: VideoPage = { items: [], total: 0, page: 1, page_size: 48 };
const EMPTY_STATS: Stats = {
  video_count: 0, archive_count: 0, total_bytes: 0, total_duration_sec: 0,
  watched_count: 0, completed_count: 0, favorite_count: 0, tag_count: 0,
  recent_days: [], top_tags: [],
};

export default function App() {
  const [ready, setReady] = useState(false);
  const [info, setInfo] = useState<LibraryInfo | null>(null);
  const [settings, setSettings] = useState<Settings>({
    library_root: null, ffmpeg_path: null, ffprobe_path: null, player_pref: "auto", mpv_path: null,
    import_root: null, double_click: "play", immersive_default: false, show_card_info: true, grid_zoom: 1,
    theme: "dark", accent: "",
  });
  const [startupError, setStartupError] = useState<string | null>(null);

  const [view, setView] = useState<ViewKey>("all");
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  // 每个页面各自记住排序（资料库各视图互不干扰）；存在 localStorage，重启也还在
  const SORT_STORE = "vv.sortByView";
  const [sortByView, setSortByView] = useState<Record<string, { sort: string; rev: boolean }>>(() => {
    try {
      return JSON.parse(localStorage.getItem(SORT_STORE) ?? "{}") as Record<string, { sort: string; rev: boolean }>;
    } catch { return {}; }
  });
  const curSort = sortByView[view] ?? { sort: "added", rev: false };
  const sort = curSort.sort;
  // 逆序开关：后端用 "-" 前缀表示取反（无前缀 = 各键默认方向）
  const sortRev = curSort.rev;
  const sortParam = sortRev ? `-${sort}` : sort;
  const setSort = useCallback((next: string | ((v: string) => string)) => {
    setSortByView((m) => {
      const prev = m[view] ?? { sort: "added", rev: false };
      const value = typeof next === "function" ? next(prev.sort) : next;
      return { ...m, [view]: { sort: value, rev: value === prev.sort ? prev.rev : false } };
    });
  }, [view]);
  const setSortRev = useCallback((next: boolean | ((v: boolean) => boolean)) => {
    setSortByView((m) => {
      const prev = m[view] ?? { sort: "added", rev: false };
      const value = typeof next === "function" ? next(prev.rev) : next;
      return { ...m, [view]: { sort: prev.sort, rev: value } };
    });
  }, [view]);
  useEffect(() => {
    try { localStorage.setItem(SORT_STORE, JSON.stringify(sortByView)); } catch { /* 忽略隐私模式 */ }
  }, [sortByView]);
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [activeCollection, setActiveCollection] = useState<string | null>(null);
  const [page, setPage] = useState(1);
  const [videoPage, setVideoPage] = useState<VideoPage>(EMPTY_PAGE);
  const [tags, setTags] = useState<Tag[]>([]);
  // 含软删除的目录，侧栏只显示未删除的，管理面板两边都要
  const [collections, setCollections] = useState<Collection[]>([]);
  const [stats, setStats] = useState<Stats>(EMPTY_STATS);

  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [detailId, setDetailId] = useState<number | null>(null);
  const [player, setPlayer] = useState<PlaybackInfo | null>(null);
  const [statsOpen, setStatsOpen] = useState(false);
  const [tagsOpen, setTagsOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [downloadOpen, setDownloadOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [dedupeOpen, setDedupeOpen] = useState(false);
  // 从详情页/批量栏带进剪辑面板的素材；面板关闭时清空，避免下次打开误带入
  const [editPrefill, setEditPrefill] = useState<VideoItem[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [bulkTagOpen, setBulkTagOpen] = useState(false);
  // 批量移动对话框：非空即打开（值为待移动的 id）
  const [moveIds, setMoveIds] = useState<number[] | null>(null);
  // 右键菜单（画廊里点卡片）：坐标 + 这次要操作的 id 列表
  const [menu, setMenu] = useState<{ x: number; y: number; ids: number[] } | null>(null);
  const [renameTarget, setRenameTarget] = useState<VideoItem | null>(null);
  const [menuBusy, setMenuBusy] = useState(false);
  // 快捷键表：默认全空，只在设置页录制后才有绑定
  const [keymap, setKeymap] = useState<KeyMap>(() => loadKeyMap());
  // 鼠标手势槽位（双击画面 / Ctrl+滚轮）：和快捷键一样存本机，改完立即生效
  const [gestures, setGestures] = useState<GestureMap>(() => loadGestures());
  // 侧栏宽度：拖得动、记得住（画匣同款，min/max 与 CSS 断点保持一致）。
  // 必须放在上面的 !ready 早退**之前** —— 0.1.26 首发把它放在早退后面，
  // ready 翻真之后 hooks 多出一个，直接 React #310 白屏（踩回了 0.1.19 的坑）。
  const [sidebarWidth, setSidebarWidth, resetSidebarWidth] = usePersistentSize(
    "vv.layout.sidebar", 236, 190, 460,
  );
  const [immersive, setImmersive] = useState(false);
  // 用户对「要不要无边框」的偏好：设置里的默认值打底。
  // 退播放只恢复窗口边框、**不清偏好**，所以下次进来还是按用户上次的选择走。
  const [immersivePref, setImmersivePref] = useState(false);
  const [toast, setToast] = useState<ToastData | null>(null);
  const [loading, setLoading] = useState(false);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [sheet, setSheet] = useState<SheetStatus>({ running: false, total: 0, done: 0, pending: 0 });
  const [sheetLive, setSheetLive] = useState<{ done: number; total: number; name?: string } | null>(null);
  const [collectionsOpen, setCollectionsOpen] = useState(false);
  // 主页层级浏览：当前所在文件夹路径（auto 收藏夹的层级 = 磁盘目录树）
  const [folderPath, setFolderPath] = useState<string[]>([]);
  const [folders, setFolders] = useState<FolderInfo[]>([]);
  const [folderSort, setFolderSort] = useState<"name" | "count" | "added">("name");
  // 手动刷新文件夹封面后自增：夹卡的 2×2 拼贴与卡片封面都靠它换版本号，绕开 WebView 图片缓存
  const [folderTick, setFolderTick] = useState(0);
  const [coverBusy, setCoverBusy] = useState(false);
  // 拖拽导入：角落浮窗的现场状态（由后端 import-progress 事件驱动）
  const [importer, setImporter] = useState<ImportStatus | null>(null);
  const [dropHover, setDropHover] = useState(false);
  // 启动屏：真实分阶段进度（打开 → 元数据 → 影片墙 → 就绪），元数据与影片墙并行谁慢等谁
  const [booting, setBooting] = useState(true);
  const [bootStage, setBootStage] = useState(0);
  const [bootFade, setBootFade] = useState(false);
  const [bootMetaDone, setBootMetaDone] = useState(false);
  const [bootGridDone, setBootGridDone] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const keymapRef = useRef(keymap);
  keymapRef.current = keymap;
  const immersiveDefaultRef = useRef(false);
  immersiveDefaultRef.current = !!settings.immersive_default;
  const immersivePrefRef = useRef(false);
  immersivePrefRef.current = immersivePref;
  const actionRef = useRef<(id: ActionId) => void>(() => {});
  const reloadRef = useRef<() => void>(() => {});
  const lastScanRefresh = useRef(0);
  // 导入结束只处理一次（防同一批完成事件重复触发扫描）
  const importFinishedRef = useRef(true);
  const doScanRef = useRef<() => void>(() => {});
  /** Shift 区间选择的锚点（上一次点中的卡片） */
  const anchorRef = useRef<number | null>(null);
  /** Ctrl+A 全选（供 keydown 用，避免把大依赖塞进事件监听） */
  const selectAllRef = useRef<() => void>(() => {});
  /** 框选：按下时的起点/基准选中集 */
  const bandStartRef = useRef<{ x: number; y: number; moved: boolean; additive: boolean } | null>(null);
  const bandBaseRef = useRef<Set<number>>(new Set());
  const [bandActive, setBandActive] = useState(false);
  /** 彻底删除：一个按钮按两次才执行（第一次进入「再点一次确认」态，几秒不点自动取消） */
  const [purgeArmed, setPurgeArmed] = useState(false);
  const purgeTimerRef = useRef<number | null>(null);
  const purgingRef = useRef(false);
  const [bandRect, setBandRect] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);

  const notify = useCallback((msg: string, kind: ToastKind = "info") => {
    setToast({ id: Date.now(), message: msg, kind });
  }, []);

  const refreshMeta = useCallback(async () => {
    const [t, c, s] = await Promise.all([api.tags(), api.collections(true), api.stats()]);
    setTags(t); setCollections(c); setStats(s);
  }, []);

  const refreshSheetStatus = useCallback(async () => {
    try { setSheet(await api.sheetStatus()); } catch { /* 库未打开时忽略 */ }
  }, []);

  // 主题：data-theme 控制整套配色，主色写进 CSS 变量；自定义色只覆盖强调色，不动明暗
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = settings.theme === "light" ? "light" : "dark";
    const custom = settings.accent ?? "";
    const accent = /^#[0-9a-fA-F]{6}$/.test(custom) ? custom : "#f97316";
    root.style.setProperty("--accent", accent);
    root.style.setProperty("--accent-soft", `${accent}26`);
  }, [settings.theme, settings.accent]);

  const activeCollections = useMemo(() => collections.filter((c) => c.deleted_at === 0), [collections]);

  // 层级浏览：加载当前层的子文件夹卡（封面来自已有缓存，点进夹后视频懒加载会逐步填满）
  useEffect(() => {
    if (!ready || !info) return;
    const parent = folderPath.length ? folderPath.join(" › ") : null;
    api.listFolderChildren(parent).then(setFolders).catch(() => setFolders([]));
  }, [ready, info, folderPath, collections, folderTick]);

  // 进入某层：文件夹路径、当前收藏夹与排序一并切换（auto 夹用拖拽位次）
  // 导航历史：结构导航（进夹/换视图/换标签/换收藏夹）都会压栈，侧键返回/前进在栈上移动
  const histRef = useRef<{ stack: NavLoc[]; idx: number }>({ stack: [], idx: -1 });
  const suppressNavRef = useRef(false);
  const navFnRef = useRef<{ back: () => void; forward: () => void }>({ back: () => {}, forward: () => {} });
  const currentLoc = (): NavLoc => ({ view, folder: folderPath, tag: activeTag, coll: activeCollection, sort, rev: sortRev });
  const pushLoc = () => {
    if (suppressNavRef.current) return;
    const h = histRef.current;
    const cur = currentLoc();
    const top = h.stack[h.idx];
    if (top && JSON.stringify(top) === JSON.stringify(cur)) return;
    h.stack = h.stack.slice(0, h.idx + 1);
    h.stack.push(cur);
    if (h.stack.length > 60) h.stack.shift();
    h.idx = h.stack.length - 1;
  };
  const applyLoc = (loc: NavLoc) => {
    setView(loc.view);
    setFolderPath(loc.folder);
    setActiveTag(loc.tag);
    setActiveCollection(loc.coll);
    setSort(loc.sort);
    setSortRev(loc.rev);
    setSelected(new Set());
  };
  const navBack = () => {
    const h = histRef.current;
    if (h.idx <= 0) return;
    suppressNavRef.current = true;
    h.idx -= 1;
    applyLoc(h.stack[h.idx]);
    suppressNavRef.current = false;
  };
  const navForward = () => {
    const h = histRef.current;
    if (h.idx >= h.stack.length - 1) return;
    suppressNavRef.current = true;
    h.idx += 1;
    applyLoc(h.stack[h.idx]);
    suppressNavRef.current = false;
  };
  useEffect(() => { navFnRef.current = { back: navBack, forward: navForward }; });
  // 鼠标侧键（主页）：返回/前进在导航历史上移动；播放页/面板/详情有各自语义，不在这里抢
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (e.button !== 3 && e.button !== 4) return;
      const t = e.target as HTMLElement;
      if (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT") return;
      e.preventDefault();
      if (player || statsOpen || tagsOpen || downloadOpen || editOpen || dedupeOpen || detailId || settingsOpen || sheetOpen || collectionsOpen) return;
      if (e.button === 3) navFnRef.current.back(); else navFnRef.current.forward();
    };
    const swallow = (e: MouseEvent) => { if (e.button === 3 || e.button === 4) e.preventDefault(); };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("auxclick", swallow);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("auxclick", swallow);
    };
  }, [player, statsOpen, tagsOpen, downloadOpen, editOpen, dedupeOpen, detailId, settingsOpen, sheetOpen, collectionsOpen]);

  const enterFolder = useCallback((path: string[]) => {
    pushLoc();
    setFolderPath(path);
    setSelected(new Set());
    // 进夹不再强制切 manual：保留用户当前排序，否则「最近加入」一进文件夹就变回入库位次，
    // 看起来像最新视频消失了。folder 排序也照旧层层保持，夹网格不会断。
    setSortRev(false);
    if (path.length) {
      setActiveCollection(path.join(" › "));
      setActiveTag(null);
      setView("all");
    } else {
      setActiveCollection(null);
    }
  }, []);

  const loadVideos = useCallback(async (reset = true) => {
    setLoading(true);
    try {
      const nextPage = reset ? 1 : page + 1;
      const result = await api.query({
        view, search: debouncedSearch || undefined, tag: activeTag ?? undefined,
        collection: activeCollection ?? undefined, sort: sortParam, page: nextPage, page_size: 48,
      });
      setPage(nextPage);
      setVideoPage((prev) => reset ? result : {
        ...result, items: [...prev.items, ...result.items],
      });
    } catch (e) {
      notify(String(e));
    } finally {
      setLoading(false);
      if (reset) setBootGridDone(true);
    }
  }, [view, debouncedSearch, activeTag, activeCollection, sort, sortParam, page, notify]);

  // 静态首帧画面（index.html 里那层不依赖 JS 的启动屏）在 React 首帧后交棒：
  // 两层同款视觉，交接无缝。App 万一挂载失败，由 main.tsx 的 5 秒保险丝兜底摘掉。
  useEffect(() => {
    document.getElementById("boot-static")?.remove();
  }, []);

  useEffect(() => {
    api.settings().then(([s, err, i]) => {
      setSettings(s); setInfo(i); setStartupError(err); setReady(true);
      if (!i) { setBooting(false); } // 没库直接进欢迎页
    }).catch((e) => { notify(String(e)); setReady(true); setBooting(false); });
  }, [notify]);

  // 启动屏阶段推进 + 保险丝（后端万一卡死 10 秒强制放行）
  useEffect(() => {
    if (ready && info) { setBootStage((s) => Math.max(s, 1)); pushLoc(); }
  }, [ready, info]);
  useEffect(() => {
    if (bootMetaDone) setBootStage((s) => Math.max(s, 2));
  }, [bootMetaDone]);
  useEffect(() => {
    if (ready && info && bootMetaDone && bootGridDone) {
      setBootStage(3);
      const t = window.setTimeout(() => {
        setBootFade(true);
        const t2 = window.setTimeout(() => setBooting(false), 320);
        return () => window.clearTimeout(t2);
      }, 200);
      return () => window.clearTimeout(t);
    }
  }, [ready, info, bootMetaDone, bootGridDone]);
  useEffect(() => {
    const t = window.setTimeout(() => setBooting(false), 10000);
    return () => window.clearTimeout(t);
  }, []);

  useEffect(() => {
    const t = window.setTimeout(() => setDebouncedSearch(search), 260);
    return () => window.clearTimeout(t);
  }, [search]);

  useEffect(() => {
    if (!ready || !info) return;
    // 换排序/筛选后回到顶部：否则窗口化列表还停在原来的滚动位置，看起来像没生效
    if (mainRef.current) mainRef.current.scrollTop = 0;
    setSelected(new Set());
    void loadVideos(true);
    void refreshMeta()
      .catch(() => {})
      .finally(() => setBootMetaDone(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, info, view, debouncedSearch, activeTag, activeCollection, sort, sortRev]);

  useEffect(() => {
    reloadRef.current = () => void loadVideos(true);
  }, [loadVideos]);

  // 无限滚动：哨兵进入视口下方 800px 就自动续载下一页；
  // loading 期间观察器先拆掉，载完重挂（列表没填满视口时会连发补齐，填满即停）
  const sentinelRef = useRef<HTMLDivElement>(null);
  const canLoadMore = videoPage.items.length < videoPage.total && !loading;
  useEffect(() => {
    if (!canLoadMore) return;
    const el = sentinelRef.current;
    if (!el) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) void loadVideos(false);
      },
      { rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [canLoadMore, loadVideos]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) => {
      return listen<SheetProgress>("sheet-progress", (e) => {
        const p = e.payload;
        if (p.finished) {
          setSheetLive(null);
          void refreshSheetStatus();
          void refreshMeta();
          reloadRef.current();
          return;
        }
        setSheetLive({ done: p.done, total: p.total, name: p.name });
      });
    }).then((fn) => {
      if (cancelled) fn(); else unlisten = fn;
    }).catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, [refreshMeta, refreshSheetStatus]);

  // 扫描进度：清点 → 识别 → 入库。写库阶段边写边刷新，能边扫边看
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) => {
      return listen<ScanProgress>("scan-progress", (e) => {
        const p = e.payload;
        if (p.phase === "done") { setScanProgress(null); return; }
        setScanProgress(p);
        if (p.phase === "write") {
          const now = Date.now();
          if (now - lastScanRefresh.current > 4000) {
            lastScanRefresh.current = now;
            reloadRef.current();
          }
        }
      });
    }).then((fn) => {
      if (cancelled) fn(); else unlisten = fn;
    }).catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  // 启动时后台整理完库（或手动整理）→ 刷新列表，让新位置立刻生效
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) => {
      return listen<number>("library-organized", (e) => {
        const moved = Number(e.payload ?? 0);
        notify(moved > 0 ? `已自动整理 ${moved} 个视频到编号文件夹` : "目录已是最新");
        void refreshMeta();
        reloadRef.current();
      });
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; }).catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, [notify, refreshMeta]);

  // 拖拽导入：系统级文件拖放（Tauri 原生事件，给的是真实文件路径），松手即入传输队列
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "enter" || p.type === "over") { setDropHover(true); return; }
        if (p.type === "leave") { setDropHover(false); return; }
        setDropHover(false);
        if (!p.paths.length) return;
        api.importVideos(p.paths)
          .then((n) => {
            if (n === 0) notify("没找到可导入的视频（支持 mp4/mkv/avi/ts/mov 等）；也可能已经在队列里了");
            else notify(`${n} 个视频已进入传输队列`);
          })
          .catch((err) => notify(String(err)));
      })
      .then((fn) => { if (cancelled) fn(); else unlisten = fn; })
      .catch((err) => notify(String(err)));
    return () => { cancelled = true; unlisten?.(); };
  }, [notify]);

  // 导入进度：角落浮窗读这个事件；一批做完自动增量扫描，拖进来就能在影片墙看到
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) =>
      listen<ImportStatus>("import-progress", (e) => {
        const s = e.payload;
        setImporter(s);
        if (s.running) { importFinishedRef.current = false; return; }
        if (s.queue_total === 0 || s.queue_done < s.queue_total) return;
        if (importFinishedRef.current) return;
        importFinishedRef.current = true;
        const ok = s.queue_done - s.failed;
        notify(s.failed > 0 ? `导入结束：成功 ${ok} 个，失败 ${s.failed} 个` : `导入完成：${ok} 个视频已就位`);
        doScanRef.current();
      }),
    ).then((fn) => { if (cancelled) fn(); else unlisten = fn; }).catch(() => {});
    // 窗口重载/刚打开时把现场捞回来：后台还在跑的话浮窗照样显示
    api.importStatus()
      .then((s) => { if (s.running && s.queue_total > 0) { setImporter(s); importFinishedRef.current = false; } })
      .catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, [notify]);

  // 做完且没失败时浮窗自己退场，别一直占角落
  useEffect(() => {
    if (!importer || importer.running || importer.failed > 0) return;
    const t = window.setTimeout(() => setImporter(null), 6000);
    return () => window.clearTimeout(t);
  }, [importer]);

  useEffect(() => {
    if (!ready || !info) return;
    void refreshSheetStatus();
  }, [ready, info, refreshSheetStatus]);

  useEffect(() => {
    if (activeCollection && !activeCollections.some((c) => c.name === activeCollection)) {
      setActiveCollection(null);
    }
  }, [activeCollections, activeCollection]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
      if (e.key === "/") { e.preventDefault(); searchRef.current?.focus(); }
      // Ctrl+A：全选当前筛选下的所有条目（不只是已加载的一屏）
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "a") {
        e.preventDefault();
        selectAllRef.current();
        return;
      }
      if (e.key === "Escape") {
        setDetailId(null); setStatsOpen(false); setTagsOpen(false); setBulkTagOpen(false);
        setSelected(new Set());
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 可录制的快捷键：默认没有绑定，录入后这里的处理器在设置页之外生效
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const combo = comboFromEvent(e);
      if (!combo) return;
      const action = actionForCombo(keymapRef.current, combo);
      if (!action) return;
      // 老板键在任何焦点下都要能按（哪怕焦点正落在搜索框里）
      if (action === "boss.hide") { e.preventDefault(); actionRef.current(action); return; }
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT" || target.isContentEditable)) return;
      e.preventDefault();
      actionRef.current(action);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const detail = useMemo(
    () => videoPage.items.find((v) => v.id === detailId) ?? null,
    [videoPage.items, detailId],
  );

  // 无边框沉浸：进播放时按「用户上次的选择」或设置默认值决定；
  // 退播放时只恢复窗口边框，不动偏好（见 restoreWindowChrome）。
  const setImmersiveMode = useCallback(async (on: boolean) => {
    setImmersivePref(on);
    setImmersive(on);
    try {
      await api.setImmersive(on);
    } catch (e) { notify(String(e)); }
  }, [notify]);

  // 退播放：把系统标题栏还回去，但不动用户的偏好
  const restoreWindowChrome = useCallback(async () => {
    setImmersive(false);
    try { await api.setImmersive(false); } catch { /* 恢复失败也别把退出流程卡住 */ }
  }, []);

  // 打开播放统一走这里：先试 mpv（支持字幕/倍速/IPC），拿不到就退回内置。
  // 首次打开与「上一条/下一条」共用同一条路径，避免翻页时上一集的 mpv 窗口留在屏幕上。
  const openById = useCallback(async (id: number) => {
    try {
      const took = await api.mpvOpen(id).catch(() => false);
      if (!took) void api.mpvClose().catch(() => {});
      const result = await api.playback(id);
      setPlayer({ ...result, source: took ? "mpv" : result.source });
      void setImmersiveMode(immersivePrefRef.current || immersiveDefaultRef.current);
      // 正在播的那条就是「当前选中的那条」，关掉播放器后网格要知道该高亮谁
      setSelected(new Set([id]));
      setDetailId((cur) => (cur === null ? null : id));
      await refreshMeta();
    } catch (e) { notify(String(e)); }
  }, [notify, refreshMeta, setImmersiveMode]);

  // 退出播放后：只更新这一条（进度/已看/元数据），并把卡片拉回视野，
  // 不整体重载第一页 —— 否则在第 N 页看的视频会被重置回第一页，选中的也就丢了
  const syncAfterPlay = useCallback(async (id: number) => {
    try {
      const fresh = await api.video(id);
      setVideoPage((prev) => ({
        ...prev,
        items: prev.items.map((it) => (it.id === id ? fresh : it)),
      }));
    } catch { /* 取不到就算了，不打断退出 */ }
    setSelected(new Set([id]));
    setDetailId((cur) => (cur === null ? null : id));
    window.requestAnimationFrame(() => {
      document
        .querySelector<HTMLElement>(`[data-video-id="${id}"]`)
        ?.scrollIntoView({ block: "nearest" });
    });
  }, []);

  const openPlayer = (video: VideoItem) => { void openById(video.id); };

  const movePlayer = async (delta: number) => {
    const items = videoPage.items;
    const idx = items.findIndex((v) => v.id === player?.video.id);
    if (idx < 0) { notify("当前视频不在当前列表里，返回列表再进"); return; }
    const next = items[idx + delta];
    if (next) { await openById(next.id); return; }
    // 到了当前已加载的尾部：先把下一页取回来，再往后翻，不用退回列表手动加载
    if (delta > 0 && items.length < videoPage.total) {
      try {
        const more = await api.query({
          view, search: debouncedSearch || undefined, tag: activeTag ?? undefined,
          collection: activeCollection ?? undefined, sort: sortParam, page: videoPage.page + 1, page_size: 48,
        });
        const merged = [...items, ...more.items];
        setVideoPage({ ...more, items: merged });
        setPage(more.page);
        const candidate = merged[idx + delta];
        if (candidate) { await openById(candidate.id); return; }
      } catch (e) { notify(String(e)); return; }
    }
    notify(delta > 0 ? "已经是最后一条" : "已经是第一条");
  };

  const doScan = async () => {
    if (!info) return;
    setScanning(true);
    setScanProgress(null);
    lastScanRefresh.current = 0;
    try {
      const report = await api.scan({ root: info.root, recursive: true, entry_limit: 60 });
      notify(
        `扫描完成：新增 ${report.added} 条，更新 ${report.updated} 条，失效 ${report.missing} 条，文件夹镜像 ${report.folders ?? 0} 个` +
        `（本次识别 ${report.probed ?? 0} 条，跳过 ${report.reused ?? 0} 条）`,
      );
      await Promise.all([loadVideos(true), refreshMeta()]);
    } catch (e) { notify(String(e)); } finally { setScanning(false); setScanProgress(null); }
  };
  // 导入完成后的自动扫描经 ref 调用，避免事件监听里捕获到旧的扫描状态
  doScanRef.current = () => void doScan();

  const startSheets = async (all: boolean) => {
    try {
      const r = await api.contactSheets(null, undefined, all);
      notify(
        `预览图：成功 ${r.done} / ${r.total}` +
        (r.cancelled ? "（已暂停，下次接着来）" : "") +
        (r.failed.length ? `，失败 ${r.failed.length} 条` : ""),
      );
      await Promise.all([loadVideos(true), refreshMeta()]);
      await refreshSheetStatus();
    } catch (e) { notify(String(e)); }
  };

  const clearSheets = async () => {
    try {
      const n = await api.clearContactSheets(null);
      notify(`已清空 ${n} 张预览图，可以重新生成`);
      await Promise.all([loadVideos(true), refreshSheetStatus()]);
    } catch (e) { notify(String(e)); }
  };

  // 文件夹视图「刷新封面」：只重置这个夹的缓存图（清自选封面 / 中帧 / 接触表记录与文件），
  // 不做全库慢扫；随后重载该夹，卡片懒加载会把封面重新抽一遍。
  const refreshFolderCovers = useCallback(async () => {
    const name = folderPath.join(" › ");
    const coll = collections.find((c) => c.deleted_at === 0 && c.kind === "auto" && c.name === name);
    if (!coll) { notify("这个文件夹还没登记进库，先点「扫描库」"); return; }
    setCoverBusy(true);
    try {
      const ids = await api.resetFolderCovers(coll.id);
      resetCovers(ids);
      setFolderTick((t) => t + 1);
      notify(`已重置 ${ids.length} 张封面，正在重新抽取…`);
      await loadVideos(true);
    } catch (e) { notify(String(e)); } finally { setCoverBusy(false); }
  }, [folderPath, collections, notify, loadVideos]);

  const scanLabel = !scanning
    ? "扫描库"
    : scanProgress
      ? `${SCAN_PHASE[scanProgress.phase]} ${scanProgress.done}/${scanProgress.total}`
      : "扫描中…";

  /** 当前列表顺序（Shift 区间选择按它算，不含还没加载的页） */
  const orderIds = useMemo(() => videoPage.items.map((v) => v.id), [videoPage.items]);

  // 选择语义对齐 Windows 资源管理器：单击=单选、Ctrl=切换单条、Shift=从锚点连选、空白拖拽=框选
  const selectOne = (video: VideoItem, ev: ReactMouseEvent) => {
    const additive = ev.ctrlKey || ev.metaKey;
    const ranged = ev.shiftKey;
    setSelected((prev) => {
      if (ranged) {
        const ids = rangeSelection(orderIds, anchorRef.current, video.id);
        const next = additive ? new Set(prev) : new Set<number>();
        for (const id of ids) next.add(id);
        return next;
      }
      if (additive) {
        const next = new Set(prev);
        if (next.has(video.id)) next.delete(video.id);
        else next.add(video.id);
        return next;
      }
      return new Set([video.id]);
    });
    anchorRef.current = video.id;
    if (!additive && !ranged) setDetailId(video.id);
  };

  // 全选：按当前筛选与排序选中**所有**匹配条目（不只已加载的那一屏）
  const selectAll = useCallback(async () => {
    try {
      const ids: number[] = [];
      for (let p = 1; p <= 50; p++) {
        const r = await api.query({
          view, search: debouncedSearch || undefined, tag: activeTag ?? undefined,
          collection: activeCollection ?? undefined, sort: sortParam, page: p, page_size: 200,
        });
        ids.push(...r.items.map((v) => v.id));
        if (ids.length >= r.total || r.items.length === 0) break;
      }
      if (!ids.length) return;
      setSelected(new Set(ids));
      notify(`已全选 ${ids.length} 条`);
    } catch (e) { notify(String(e), "error"); }
  }, [view, debouncedSearch, activeTag, activeCollection, sortParam, notify]);
  selectAllRef.current = () => { void selectAll(); };

  // 库内回收站：只改库标记，磁盘文件不动；移入不提示（回收站里可「恢复」反悔）
  const trashIds = async (ids: number[]) => {
    if (!ids.length) return;
    try {
      for (const id of ids) await api.trash(id, true);
      notify(`已移入回收站 ${ids.length} 条（文件原样保留）`);
      setSelected(new Set());
      setDetailId(null);
      setMenu(null);
      await Promise.all([refreshMeta(), loadVideos(true)]);
    } catch (e) { notify(String(e)); }
  };

  // 彻底删除：连磁盘文件一起删，不可恢复。只对回收站里的条目开放（后端还会再校验一次）。
  // 一个按钮按两次：第一次进入「再点一次确认」态，几秒不点自动取消。
  const purgeIds = async (ids: number[]) => {
    if (!ids.length || purgingRef.current) return;
    if (!purgeArmed) {
      setPurgeArmed(true);
      if (purgeTimerRef.current) window.clearTimeout(purgeTimerRef.current);
      purgeTimerRef.current = window.setTimeout(() => setPurgeArmed(false), 4000);
      return;
    }
    if (purgeTimerRef.current) window.clearTimeout(purgeTimerRef.current);
    purgeTimerRef.current = null;
    setPurgeArmed(false);
    purgingRef.current = true;
    try {
      const report = await api.purgeVideos(ids);
      if (report.failed.length) console.warn("彻底删除跳过的条目：", report.failed);
      const skipped = report.failed.length ? `，${report.failed.length} 条被跳过` : "";
      notify(
        `已彻底删除 ${report.deleted} 个，释放 ${formatBytes(report.bytes)}${skipped}`,
        report.failed.length ? "error" : "success",
      );
      setSelected(new Set());
      setDetailId(null);
      setMenu(null);
      await Promise.all([refreshMeta(), loadVideos(true)]);
    } catch (e) {
      notify(String(e), "error");
    } finally {
      purgingRef.current = false;
    }
  };

  // 动作表：右键菜单与可录制快捷键共用同一套入口
  const runAction = (id: ActionId) => {
    const ids = selected.size ? [...selected] : detailId != null ? [detailId] : [];
    switch (id) {
      case "sort.added": case "sort.folder": case "sort.title": case "sort.size":
        setSort(id.slice(5));
        setSortRev(false);
        if (id === "sort.folder") setFolderPath([]);
        break;
      case "sort.toggleDir":
        setSortRev((v) => !v);
        break;
      case "play.toggle": case "play.speedUp": case "play.speedDown":
        if (!player) { notify("先在画廊里双击一条开始播放"); break; }
        window.dispatchEvent(new CustomEvent("vv-player-action", { detail: { action: id } }));
        break;
      case "select.all":
        setSelected(new Set(videoPage.items.map((v) => v.id)));
        break;
      case "select.clear":
        setSelected(new Set());
        break;
      case "fav.toggle": {
        if (!ids.length) { notify("先勾选影片，或打开一条详情"); break; }
        const first = videoPage.items.find((v) => v.id === ids[0]);
        const fav = !first?.favorite;
        void (async () => {
          try {
            for (const videoId of ids) await api.patch({ id: videoId, favorite: fav });
            notify(fav ? `已收藏 ${ids.length} 条` : `已取消收藏 ${ids.length} 条`);
            await Promise.all([refreshMeta(), loadVideos(true)]);
          } catch (e) { notify(String(e)); }
        })();
        break;
      }
      case "folder.up":
        if (!folderPath.length) { notify("已经在库根目录了"); break; }
        setFolderPath((p) => p.slice(0, -1));
        break;
      case "edit.rename": {
        const target = videoPage.items.find((v) => v.id === ids[0]) ?? detail;
        if (!target) { notify("先勾选一条影片"); break; }
        setRenameTarget(target);
        break;
      }
      case "edit.move":
        if (!ids.length) { notify("先勾选要移动的影片"); break; }
        setMoveIds(ids);
        break;
      case "edit.trash":
        void trashIds(ids);
        break;
      case "immersive.toggle":
        void setImmersiveMode(!immersive);
        break;
      case "boss.hide":
        // 老板键：把窗口整个藏起来。恢复靠系统级全局热键（Rust 注册）。
        void api.hideMainWindow().catch((e) => notify(String(e)));
        break;
      case "boss.recall":
        // 呼回键是系统级热键，窗口藏起来之后前端收不到按键，这里按了也没用
        notify("呼回键是系统级热键，请在设置里改绑定");
        break;
    }
    setMenu(null);
  };
  actionRef.current = runAction;

  // 收藏夹内拖拽排序：进收藏夹视图自动切「手动排序」，卡片可拖拽换位
  const activeCollectionId = useMemo(
    () => collections.find((c) => c.deleted_at === 0 && c.name === activeCollection)?.id ?? null,
    [collections, activeCollection],
  );
  const dragEnabled = !!(activeCollection && activeCollectionId && sort === "manual");
  const dragVideoRef = useRef<number | null>(null);
  const onCardDragStart = (video: VideoItem, e: DragEvent<HTMLElement>) => {
    dragVideoRef.current = video.id;
    e.dataTransfer.effectAllowed = "move";
  };
  const onCardDragOver = (video: VideoItem, e: DragEvent<HTMLElement>) => {
    if (dragVideoRef.current == null || dragVideoRef.current === video.id) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    e.currentTarget.classList.add("drag-over");
  };
  const onCardDragLeave = (e: DragEvent<HTMLElement>) => e.currentTarget.classList.remove("drag-over");
  const onCardDrop = (video: VideoItem, e: DragEvent<HTMLElement>) => {
    e.currentTarget.classList.remove("drag-over");
    const dragId = dragVideoRef.current;
    dragVideoRef.current = null;
    if (!dragEnabled || dragId == null || dragId === video.id || !activeCollectionId) return;
    e.preventDefault();
    const items = [...videoPage.items];
    const from = items.findIndex((it) => it.id === dragId);
    const to = items.findIndex((it) => it.id === video.id);
    if (from < 0 || to < 0) return;
    const [moved] = items.splice(from, 1);
    items.splice(to, 0, moved);
    setVideoPage((prev) => ({ ...prev, items }));
    void api
      .reorderCollectionVideos(activeCollectionId, items.map((it) => it.id))
      .then(() => notify("收藏夹顺序已保存"))
      .catch((err) => notify(String(err)));
  };
  const onCardDragEnd = (e: DragEvent<HTMLElement>) => e.currentTarget.classList.remove("drag-over");

  const chooseLibrary = async () => {
    try {
      const dir = await api.pickDirectory();
      if (!dir) return;
      const i = await api.setLibrary(dir);
      setInfo(i);
      setSettings((s) => ({ ...s, library_root: dir }));
      setStartupError(null);
      notify(`已打开视频库：${dir}`);
    } catch (e) { notify(String(e)); }
  };

  // ── 大列表窗口化：只渲染视口附近的卡片，DOM 规模不再随滚动无限增长 ──
  // 注意：这些 hook 必须放在 !ready / !info 早退之前（0.1.19 黑屏坑）。
  const mainRef = useRef<HTMLElement | null>(null);
  const gridRef = useRef<HTMLDivElement | null>(null);
  const [gridWin, setGridWin] = useState({ start: 0, end: 96 });
  const [gridMetrics, setGridMetrics] = useState({ cols: 4, rowH: 250 });

  // ── 影片墙缩放：一把控制卡片最小宽度，窗口化行高跟着一起算 ──
  // 208px 是 0.1.25 定下的基准尺寸，缩放以它为 1.0 原点，避免改一次要重算历史布局。
  const zoom = settings.grid_zoom ?? 1;
  const cardMin = Math.round(GRID_CARD_BASE * zoom);
  const gridGap = Math.max(6, Math.round(GRID_GAP_BASE * zoom));
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const zoomSaveTimer = useRef<number | null>(null);
  const applyZoom = useCallback((next: number) => {
    const z = Math.min(GRID_ZOOM_MAX, Math.max(GRID_ZOOM_MIN, Math.round(next * 100) / 100));
    setSettings((s) => (s.grid_zoom === z ? s : { ...s, grid_zoom: z }));
    // 连续拖动时不必每次都落盘，停手 400ms 再写
    if (zoomSaveTimer.current) window.clearTimeout(zoomSaveTimer.current);
    zoomSaveTimer.current = window.setTimeout(() => {
      void api.updateSettings({ ...settingsRef.current, grid_zoom: z }).catch(() => {});
    }, 400);
  }, []);
  useEffect(() => () => { if (zoomSaveTimer.current) window.clearTimeout(zoomSaveTimer.current); }, []);

  const calcGridMetrics = useCallback(() => {
    const main = mainRef.current;
    if (!main) return null;
    const chrome = window.innerWidth <= 900 ? 36 : 56;
    const avail = Math.max(cardMin, main.clientWidth - chrome);
    const cols = Math.max(1, Math.floor((avail + gridGap) / (cardMin + gridGap)));
    const cardW = Math.max(cardMin, (avail - (cols - 1) * gridGap) / cols);
    const rowH = Math.round(cardW * 9 / 16 + (settings.show_card_info === false ? 0 : 78) + gridGap);
    return { cols, rowH };
  }, [settings.show_card_info, cardMin, gridGap]);
  const updateGridWindow = useCallback(() => {
    const main = mainRef.current;
    const grid = gridRef.current;
    const m = calcGridMetrics();
    if (!main || !grid || !m) return;
    const total = videoPage.items.length;
    const rowCount = Math.ceil(total / m.cols);
    const offset = main.scrollTop - grid.offsetTop;
    const firstRow = Math.max(0, Math.floor(offset / m.rowH) - 5);
    const lastRow = Math.min(rowCount, Math.ceil((offset + main.clientHeight) / m.rowH) + 5);
    const next = {
      start: firstRow * m.cols,
      end: Math.min(total, Math.max(firstRow * m.cols + m.cols, lastRow * m.cols)),
    };
    setGridMetrics(m);
    setGridWin((prev) => (prev.start === next.start && prev.end === next.end ? prev : next));
  }, [calcGridMetrics, videoPage.items.length]);
  useEffect(() => {
    updateGridWindow();
    const main = mainRef.current;
    if (!main) return;
    const onScroll = () => updateGridWindow();
    main.addEventListener("scroll", onScroll, { passive: true });
    const ro = new ResizeObserver(() => updateGridWindow());
    ro.observe(main);
    return () => { main.removeEventListener("scroll", onScroll); ro.disconnect(); };
  }, [updateGridWindow, view, folderPath, sort]);

  // Ctrl + 滚轮调卡片大小。只在按住 Ctrl 时接管，普通滚轮照常滚动，
  // 避免出现画匣里那种「两个 wheel 处理器抢事件」的问题。
  useEffect(() => {
    const main = mainRef.current;
    if (!main) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      const dir = e.deltaY > 0 ? -1 : 1;
      applyZoom((settingsRef.current.grid_zoom ?? 1) + dir * GRID_ZOOM_STEP);
    };
    main.addEventListener("wheel", onWheel, { passive: false });
    return () => main.removeEventListener("wheel", onWheel);
  }, [applyZoom, ready]);

  // 空白处按住拖动 = 框选（Windows 资源管理器同款）：拖到视口上下边缘会自动滚动
  useEffect(() => {
    if (!bandActive) return;
    const start = bandStartRef.current;
    if (!start) return;
    const onMove = (e: MouseEvent) => {
      const dx = e.clientX - start.x;
      const dy = e.clientY - start.y;
      if (!start.moved && Math.hypot(dx, dy) > 4) start.moved = true;
      setBandRect({ x0: start.x, y0: start.y, x1: e.clientX, y1: e.clientY });
      if (!start.moved) return;
      const grid = gridRef.current;
      const main = mainRef.current;
      if (!grid) return;
      const box = rectFromPoints(start.x, start.y, e.clientX, e.clientY);
      const hit: number[] = [];
      grid.querySelectorAll<HTMLElement>("[data-video-id]").forEach((node) => {
        const r = node.getBoundingClientRect();
        if (rectsIntersect(box, { left: r.left, top: r.top, right: r.right, bottom: r.bottom })) {
          const id = Number(node.getAttribute("data-video-id"));
          if (Number.isFinite(id)) hit.push(id);
        }
      });
      // Ctrl 框选 = 在原有选择上追加；普通框选 = 只保留框中的
      setSelected(new Set([...bandBaseRef.current, ...hit]));
      if (main) {
        const mr = main.getBoundingClientRect();
        if (e.clientY > mr.bottom - 56) main.scrollTop += 22;
        else if (e.clientY < mr.top + 56) main.scrollTop -= 22;
      }
    };
    const onUp = () => {
      // 只是点了一下空白处（没拖动）→ 清空选择，跟资源管理器一致
      if (start && !start.moved && !start.additive) setSelected(new Set());
      setBandActive(false);
      setBandRect(null);
      bandStartRef.current = null;
      bandBaseRef.current = new Set();
      document.body.classList.remove("banding");
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    // 切走窗口（Alt+Tab）时收不到 mouseup，兜底收尾，别让遮罩卡在屏幕上
    window.addEventListener("blur", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      window.removeEventListener("blur", onUp);
      document.body.classList.remove("banding");
    };
  }, [bandActive]);

  // 「最近入库」首屏后台预热：错开 180ms 串行抽帧，不让新片卡在占位图上
  useEffect(() => {
    if (!ready || !info || view !== "latest") return;
    const ids = videoPage.items.slice(0, 24).filter((v) => !v.cover).map((v) => v.id);
    if (!ids.length) return;
    const t = window.setTimeout(() => { void prewarmCovers(ids); }, 1200);
    return () => window.clearTimeout(t);
  }, [ready, info, view, videoPage.items]);

  const virtualized = videoPage.items.length > 96;
  const visibleItems = virtualized
    ? videoPage.items.slice(Math.min(gridWin.start, videoPage.items.length), Math.min(gridWin.end, videoPage.items.length))
    : videoPage.items;
  const topPad = virtualized
    ? Math.floor(Math.min(gridWin.start, videoPage.items.length) / gridMetrics.cols) * gridMetrics.rowH
    : 0;
  const bottomPad = virtualized
    ? Math.max(0, Math.ceil((videoPage.items.length - gridWin.end) / gridMetrics.cols) * gridMetrics.rowH)
    : 0;

  if (!ready) {
    return <BootSplash stage={0} fading={false} />;
  }

  if (!info) {
    return (
      <div className="welcome">
        <div className="welcome-card">
          <div className="welcome-mark"><Icon name="vault" size={40} /></div>
          <h1>欢迎使用影匣</h1>
          <p>把散落在硬盘各处的视频和压缩包，收进一个离线、可检索、有记忆的私人片库。</p>
          {startupError && <div className="warning">{startupError}</div>}
          <button className="btn primary big" onClick={chooseLibrary}>
            <Icon name="folder" size={18} /> 选择库目录
          </button>
          <div className="welcome-tips">
            <span><Icon name="check" size={14} /> 全部数据只在本机</span>
            <span><Icon name="check" size={14} /> 支持 ZIP / CBZ 内视频逐条管理</span>
            <span><Icon name="check" size={14} /> 自动记住看到第几秒</span>
          </div>
          <button className="link-btn" onClick={() => setSettingsOpen(true)}>高级设置</button>
        </div>
        {settingsOpen && <SettingsModal settings={settings} setSettings={setSettings} keymap={keymap} setKeymap={setKeymap} gestures={gestures} setGestures={setGestures} onClose={() => setSettingsOpen(false)} notify={notify} />}
      </div>
    );
  }

  const showPanel = statsOpen || tagsOpen || downloadOpen || editOpen || dedupeOpen;

  return (
    <div className="app-shell">
      <Sidebar
        width={sidebarWidth}
        view={view}
        onView={(v) => {
          pushLoc();
          setView(v); setActiveTag(null); setActiveCollection(null); setFolderPath([]);
          // 「最近入库」是固定语义：跨所有文件夹、按真实入库顺序，不能沿用上一个视图的排序
          if (v === "latest") { setSort("added"); setSortRev(false); }
          setStatsOpen(false); setTagsOpen(false); setDownloadOpen(false); setEditOpen(false); setDedupeOpen(false); setSelected(new Set());
        }}
        tags={tags}
        activeTag={activeTag}
        onTag={(t) => { pushLoc(); setActiveTag(t); setActiveCollection(null); setView("all"); setStatsOpen(false); setTagsOpen(false); }}
        collections={activeCollections}
        activeCollection={activeCollection}
        onCollection={(c) => {
          const target = collections.find((x) => x.deleted_at === 0 && x.name === c);
          if (target && target.kind === "auto") { enterFolder(c ? c.split(" › ") : []); return; }
          pushLoc();
          setFolderPath([]);
          setActiveCollection(c); setActiveTag(null); setView("all"); setStatsOpen(false); setTagsOpen(false); setSort(c ? "manual" : "added"); setSortRev(false);
        }}
        onCreateCollection={() => setCollectionsOpen(true)}
        onManageCollections={() => setCollectionsOpen(true)}
        onReordered={() => void refreshMeta()}
        onStats={() => { setStatsOpen(true); setTagsOpen(false); setDownloadOpen(false); setEditOpen(false); setDedupeOpen(false); }}
        onTags={() => { setTagsOpen(true); setStatsOpen(false); setDownloadOpen(false); setEditOpen(false); setDedupeOpen(false); }}
        onDownload={() => { setDownloadOpen(true); setStatsOpen(false); setTagsOpen(false); setEditOpen(false); setDedupeOpen(false); }}
        onEdit={() => { setEditOpen(true); setStatsOpen(false); setTagsOpen(false); setDownloadOpen(false); setDedupeOpen(false); }}
        onSettings={() => setSettingsOpen(true)}
        statsOpen={statsOpen}
        tagsOpen={tagsOpen}
        downloadOpen={downloadOpen}
        editOpen={editOpen}
      />

      <Splitter
        value={sidebarWidth}
        min={190}
        max={460}
        onChange={setSidebarWidth}
        onReset={resetSidebarWidth}
        label="调整侧栏宽度"
      />

      <main className="main" ref={mainRef}>
        {showPanel ? (
          statsOpen ? <StatsPanel stats={stats} onClose={() => setStatsOpen(false)} onOpenVideo={(id) => {
            setStatsOpen(false);
            void openById(id);
          }} />
            : dedupeOpen ? <DedupePanel onClose={() => setDedupeOpen(false)}
                onChanged={() => { void refreshMeta(); void loadVideos(true); }} notify={notify} />
            : downloadOpen ? <DownloadPanel tags={tags} onClose={() => setDownloadOpen(false)}
                onImported={() => { void refreshMeta(); void loadVideos(true); }} notify={notify} />
            : editOpen ? <EditPanel prefill={editPrefill}
                onClose={() => { setEditOpen(false); setEditPrefill(null); }}
                onImported={() => { void refreshMeta(); void loadVideos(true); }} notify={notify} />
            : <TagManager tags={tags} onClose={() => setTagsOpen(false)} onChanged={() => { void refreshMeta(); void loadVideos(true); }} onError={notify} />
        ) : (
          <>
            <header className="toolbar">
              <div className="search-box">
                <Icon name="search" size={17} />
                <input
                  ref={searchRef}
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder="搜文件名、备注、标签…  按 / 聚焦"
                />
                {search && <button className="clear-search" onClick={() => setSearch("")}><Icon name="x" size={14} /></button>}
              </div>
              <select className="select" value={sort} onChange={(e) => { setSort(e.target.value); setSortRev(false); }} title="排序">
                {SORTS.map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
              </select>
              <button
                className={`icon-btn sort-dir ${sortRev ? "rev" : ""}`}
                onClick={() => setSortRev((v) => !v)}
                title={sortRev ? "当前：逆序（点一下改成正序）" : "当前：正序（点一下改成逆序）"}
              >
                <Icon name="sortDir" size={16} />
              </button>
              <button className="btn" onClick={() => void selectAll()} title="全选当前筛选下的所有视频（Ctrl+A）；也可在空白处拖拽框选、Shift 连选">
                全选
              </button>
              <button className="btn" onClick={doScan} disabled={scanning} title="增量扫描：已登记且没变过的文件不会再跑 ffprobe">
                <Icon name="refresh" size={16} className={scanning ? "spin" : ""} />{scanLabel}
              </button>
              <button className="btn" onClick={() => { setSheetOpen(true); void refreshSheetStatus(); }} title="批量生成预览图：可暂停、可续跑、可清空重建">
                <Icon name="camera" size={16} className={sheet.running ? "spin" : ""} />
                {sheetLive ? `${sheetLive.done}/${sheetLive.total}` : sheet.pending > 0 ? `预览图 缺 ${sheet.pending}` : "预览图"}
              </button>
              <button className="icon-btn" onClick={() => { setDedupeOpen(true); setStatsOpen(false); setTagsOpen(false); setDownloadOpen(false); setEditOpen(false); }}
                title="重复清理：按内容指纹找同一份视频的多份副本"><Icon name="merge" size={17} /></button>
              <div className="zoom-ctl" title="卡片大小：点 +/-，或按住 Ctrl 滚滚轮">
                <button className="zoom-btn" onClick={() => applyZoom(zoom - GRID_ZOOM_STEP)}
                  disabled={zoom <= GRID_ZOOM_MIN + 1e-6} title="缩小卡片">−</button>
                <button className="zoom-val" onClick={() => applyZoom(1)}
                  title={`当前 ${Math.round(zoom * 100)}%，点一下回到 100%`}>{Math.round(zoom * 100)}%</button>
                <button className="zoom-btn" onClick={() => applyZoom(zoom + GRID_ZOOM_STEP)}
                  disabled={zoom >= GRID_ZOOM_MAX - 1e-6} title="放大卡片">+</button>
              </div>
              <button className="icon-btn" onClick={() => setSettingsOpen(true)} title="设置"><Icon name="settings" size={18} /></button>
            </header>

            <div className="result-bar">
              <div>
                <strong>{videoPage.total.toLocaleString()}</strong> 条视频
                {(activeTag || activeCollection) && (
                  <span className="filter-chip">{activeTag ? `标签：${activeTag}` : `收藏夹：${activeCollection}`}
                    <button onClick={() => { pushLoc(); setActiveTag(null); setActiveCollection(null); setFolderPath([]); setSort("added"); setSortRev(false); }}><Icon name="x" size={12} /></button>
                  </span>
                )}
              </div>
              <div className="result-right">
                <span className="muted">库：{info.root}</span>
                <span className="size-chip">{formatBytes(stats.total_bytes)}</span>
              </div>
            </div>

            {folderPath.length > 0 && (
              <nav className="breadcrumb">
                <button className={folderPath.length === 0 ? "cur" : ""} onClick={() => enterFolder([])}>全部视频</button>
                {folderPath.map((seg, i) => (
                  <span key={folderPath.slice(0, i + 1).join(" › ")} className="crumb-seg">
                    <span className="crumb-sep">›</span>
                    <button className={i === folderPath.length - 1 ? "cur" : ""} onClick={() => enterFolder(folderPath.slice(0, i + 1))}>{seg}</button>
                  </span>
                ))}
                <button
                  className="btn small crumb-refresh"
                  onClick={() => void refreshFolderCovers()}
                  disabled={coverBusy}
                  title="只重抽这个文件夹里的封面（自选封面 / 中帧 / 接触表一起重置），不做全库慢扫"
                >
                  <Icon name="refresh" size={13} className={coverBusy ? "spin" : ""} />{coverBusy ? "刷新中…" : "刷新封面"}
                </button>
              </nav>
            )}

            {view === "gallery" && (
              <section className="folder-strip gallery-strip">
                <div className="folder-strip-head">
                  <span className="section-title">合集画廊 · 只记名与统计，影片仍逐条显示</span>
                  <span className="gallery-head-actions">
                    <button className={`pill ${activeCollection ? "" : "on"}`} onClick={() => { setActiveCollection(null); setSort("added"); setSortRev(false); }}>全部影片</button>
                    <button className="pill" onClick={() => setCollectionsOpen(true)}><Icon name="plus" size={12} />新建合集</button>
                  </span>
                </div>
                <div className="folder-grid">
                  {activeCollections.filter((c) => c.kind === "custom").map((c) => {
                    const covers = videoPage.items.filter((v) => v.collections.includes(c.name))
                      .map((v) => v.cover || v.contact_sheet).filter(Boolean).slice(0, 4);
                    return (
                      <button
                        key={c.id}
                        className={`folder-card ${activeCollection === c.name ? "active" : ""}`}
                        onClick={() => { setActiveCollection((cur) => (cur === c.name ? null : c.name)); setSort("added"); setSortRev(false); }}
                        title={`${c.name} · ${c.count} 条${c.group_name ? ` · ${c.group_name}` : ""}`}
                      >
                        <div className={`folder-thumb c${covers.length}`}>
                          {covers.length > 0
                            ? covers.map((src, i) => <img key={i} src={src} alt="" loading="lazy" />)
                            : <div className="folder-thumb-empty"><Icon name="sparkle" size={26} /></div>}
                          <span className="folder-count">{c.count}</span>
                        </div>
                        <div className="folder-name truncate">{c.name}</div>
                      </button>
                    );
                  })}
                  {activeCollections.filter((c) => c.kind === "custom").length === 0 && (
                    <p className="folder-mode-hint">还没有合集。点「新建合集」起一个名字，影片照常逐条展示。</p>
                  )}
                </div>
              </section>
            )}

            {sort === "folder" && (
              <section className="folder-strip">
                <div className="folder-strip-head">
                  <span className="section-title">{folderPath.length ? "子文件夹" : "文件夹"}</span>
                  <span className="folder-sort">
                    <button className={folderSort === "name" ? "on" : ""} onClick={() => setFolderSort("name")}>名称</button>
                    <button className={folderSort === "count" ? "on" : ""} onClick={() => setFolderSort("count")}>数量</button>
                    <button className={folderSort === "added" ? "on" : ""} onClick={() => setFolderSort("added")} title="按夹内最新入库时间排，找新片最快">最新入库</button>
                  </span>
                </div>
                <div className="folder-grid">
                  {folders.length === 0 ? (
                    <p className="folder-mode-hint">这一层没有子文件夹了，下面显示本层影片。</p>
                  ) : [...folders]
                    .sort((a, b) => (folderSort === "name"
                      ? a.leaf.localeCompare(b.leaf, "zh-CN")
                      : folderSort === "added"
                        ? b.latest_added - a.latest_added
                        : b.count - a.count))
                    .map((f) => (
                      <button className="folder-card" key={f.id} onClick={() => enterFolder([...folderPath, f.leaf])}
                        title={f.name + " · " + f.count + " 条"}>
                        <div className={`folder-thumb c${Math.min(f.covers.length, 4)}`}>
                          {f.covers.length > 0
                            ? f.covers.map((c, i) => <img key={i} src={convertFileSrc(c) + (folderTick ? `?v=${folderTick}` : "")} alt="" loading="lazy" />)
                            : <div className="folder-thumb-empty"><Icon name="folder" size={26} /></div>}
                          <span className="folder-count">{f.count}</span>
                        </div>
                        <div className="folder-name truncate">{f.leaf}</div>
                      </button>
                    ))}
                </div>
              </section>
            )}

            {selected.size > 0 && (
              <div className="bulk-bar">
                <span>已选 <b>{selected.size}</b> 条</span>
                {view === "trash" ? (
                  <>
                    <button className="btn small" onClick={async () => {
                      try {
                        for (const id of selected) await api.trash(id, false);
                        notify(`已恢复 ${selected.size} 条`);
                        setSelected(new Set());
                        await Promise.all([refreshMeta(), loadVideos(true)]);
                      } catch (e) { notify(String(e), "error"); }
                    }}><Icon name="check" size={14} />恢复</button>
                    <button className="btn small danger" onClick={() => void purgeIds([...selected])}>
                      <Icon name="trash" size={14} />{purgeArmed ? "再点一次确认" : `彻底删除（${selected.size}）`}
                    </button>
                    <span className="muted">彻底删除会连磁盘文件一起删掉，不可恢复</span>
                  </>
                ) : (
                  <>
                <button className="btn small" onClick={() => setBulkTagOpen((v) => !v)}><Icon name="tag" size={14} />批量打标</button>
                <button
                  className="btn small"
                  disabled={selected.size < 2}
                  title={selected.size < 2 ? "拼接至少要选 2 条（按当前列表顺序拼接）" : "把所选视频按列表顺序拼接成一条"}
                  onClick={() => {
                    setEditPrefill(videoPage.items.filter((it) => selected.has(it.id)));
                    setEditOpen(true);
                  }}
                ><Icon name="merge" size={14} />拼接所选</button>
                <button className="btn small" onClick={async () => {
                  try {
                    const ids = [...selected];
                    for (const id of ids) await api.setCollection("待看", id, true);
                    notify(`已把 ${ids.length} 条加入待看`); await refreshMeta();
                  } catch (e) { notify(String(e)); }
                }}><Icon name="list" size={14} />加入待看</button>
                <button
                  className="btn small"
                  onClick={() => setMoveIds([...selected])}
                  title="把所选视频移动到库内另一个文件夹（同盘瞬时完成，封面不重抽）"
                >
                  <Icon name="moveIn" size={14} />移动到…
                </button>
                <select className="select small-select" value="" onChange={async (e) => {
                  const id = Number(e.target.value);
                  if (!id) return;
                  try {
                    const ids = [...selected];
                    for (const v of ids) await api.setCollection(id, v, true);
                    notify(`已把 ${ids.length} 条加入收藏夹`);
                    await Promise.all([refreshMeta(), loadVideos(true)]);
                  } catch (err) { notify(String(err)); }
                  e.target.value = "";
                }} title="批量加入收藏夹">
                  <option value="">加入收藏夹…</option>
                  {activeCollections.filter((c) => c.kind !== "auto").map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
                </select>
                  </>
                )}
                <button className="btn small" onClick={() => setSelected(new Set())}>取消选择</button>
                {bulkTagOpen && (
                  <div className="bulk-tags">
                    {tags.map((t) => (
                      <button key={t.id} className="pill" onClick={async () => {
                        try {
                          const n = await api.bulkTag([...selected], [t.id], true);
                          notify(`已给 ${n} 条打上「${t.name}」`);
                          await Promise.all([loadVideos(true), refreshMeta()]);
                        } catch (e) { notify(String(e)); }
                      }}>{t.name}</button>
                    ))}
                    {tags.length === 0 && <span className="muted">先在标签中心建标签</span>}
                  </div>
                )}
              </div>
            )}

            {sort === "folder" && folders.length > 0 ? (
              <p className="folder-mode-hint">
                当前是「文件夹」排序：这一层只列文件夹，点进去看视频；换成其它排序就会回到影片墙。
              </p>
            ) : videoPage.items.length === 0 ? (
              <div className="empty">
                <Icon name="grid" size={40} />
                <h2>这里还是空的</h2>
                <p>点右上角「扫描库」，影匣会把视频和压缩包里的视频逐条登记进来。</p>
                <button className="btn primary" onClick={doScan} disabled={scanning}>
                  <Icon name="refresh" size={16} />扫描 {info.root}
                </button>
              </div>
            ) : (
              <>
                <div
                  className="video-grid"
                  ref={gridRef}
                  style={{ "--card-min": `${cardMin}px`, "--grid-gap": `${gridGap}px` } as CSSProperties}
                  onMouseDown={(e) => {
                    // 只在空白处起手：卡片自己的点击/拖拽不受影响
                    if (e.button !== 0) return;
                    if ((e.target as HTMLElement).closest("[data-video-id]")) return;
                    e.preventDefault();
                    const additive = e.ctrlKey || e.metaKey;
                    bandStartRef.current = { x: e.clientX, y: e.clientY, moved: false, additive };
                    bandBaseRef.current = additive ? new Set(selected) : new Set();
                    setBandRect({ x0: e.clientX, y0: e.clientY, x1: e.clientX, y1: e.clientY });
                    setBandActive(true);
                    document.body.classList.add("banding");
                  }}
                  onContextMenu={(e) => {
                    const el = (e.target as HTMLElement).closest("[data-video-id]");
                    if (!el) return;
                    e.preventDefault();
                    const id = Number(el.getAttribute("data-video-id"));
                    if (!Number.isFinite(id)) return;
                    const ids = selected.has(id) ? [...selected] : [id];
                    if (!selected.has(id)) setSelected(new Set([id]));
                    setMenu({ x: e.clientX, y: e.clientY, ids });
                  }}
                >
                  {topPad > 0 && <div className="grid-spacer" style={{ height: topPad }} aria-hidden />}
                  {visibleItems.map((v) => (
                    <VideoCard
                      key={`${v.id}`}
                      video={v}
                      selected={selected.has(v.id)}
                      doubleClick={settings.double_click === "detail" ? "detail" : "play"}
                      showInfo={settings.show_card_info !== false}
                      onSelect={selectOne}
                      onOpen={(item) => setDetailId(item.id)}
                      onPlay={(item) => void openPlayer(item)}
                      dragEnabled={dragEnabled}
                      onDragStart={onCardDragStart}
                      onDragOver={onCardDragOver}
                      onDragLeave={onCardDragLeave}
                      onDrop={onCardDrop}
                      onDragEnd={onCardDragEnd}
                    />
                  ))}
                  {bottomPad > 0 && <div className="grid-spacer" style={{ height: bottomPad }} aria-hidden />}
                </div>
                {videoPage.items.length < videoPage.total && (
                  <div className="load-more" ref={sentinelRef}>
                    {loading ? (
                      <span className="muted">加载中…</span>
                    ) : (
                      <button className="btn" onClick={() => void loadVideos(false)}>
                        加载更多（还有 {videoPage.total - videoPage.items.length} 条）
                      </button>
                    )}
                  </div>
                )}
              </>
            )}
          </>
        )}
      </main>

      {detail && !player && !showPanel && (
        <DetailDrawer
          video={detail}
          tags={tags}
          collections={activeCollections}
          onClose={() => setDetailId(null)}
          onPlay={(v) => void openPlayer(v)}
          onEdit={(v) => { setEditPrefill([v]); setEditOpen(true); setDetailId(null); }}
          onChanged={() => { void refreshMeta(); void loadVideos(true); }}
          onVideoUpdated={(v) => {
            // 就地替换这一张卡：改名不重载列表，滚动位置和分页都不动
            setVideoPage((prev) => ({ ...prev, items: prev.items.map((it) => (it.id === v.id ? v : it)) }));
            void refreshMeta();
          }}
          onError={notify}
          notify={notify}
        />
      )}

      {menu && (
        <>
          <div className="menu-shadow" onMouseDown={() => setMenu(null)} onContextMenu={(e) => { e.preventDefault(); setMenu(null); }} />
          <div
            className="context-menu"
            style={{ left: Math.max(8, Math.min(menu.x, window.innerWidth - 224)), top: Math.max(8, Math.min(menu.y, window.innerHeight - 470)) }}
          >
            <div className="ctx-head">{menu.ids.length > 1 ? `已选 ${menu.ids.length} 条` : videoPage.items.find((v) => v.id === menu.ids[0])?.file_name ?? "影片"}</div>
            {menu.ids.length === 1 && (
              <>
                <button onClick={() => { setDetailId(menu.ids[0]); setMenu(null); }}><Icon name="info" size={14} />查看详情</button>
                <button onClick={() => { const id = menu.ids[0]; setMenu(null); void openById(id); }}><Icon name="play" size={14} />直接播放</button>
                <button onClick={() => runAction("edit.rename")}><Icon name="edit" size={14} />重命名</button>
                <button onClick={() => runAction("fav.toggle")}><Icon name="heart" size={14} />收藏 / 取消收藏</button>
                <button disabled={menuBusy} onClick={async () => {
                  setMenuBusy(true);
                  try { await api.revealVideo(menu.ids[0]); setMenu(null); }
                  catch (e) { notify(String(e)); } finally { setMenuBusy(false); }
                }}><Icon name="folder" size={14} />在资源管理器中定位</button>
              </>
            )}
            <button onClick={() => runAction("edit.move")}><Icon name="moveIn" size={14} />移动到文件夹…</button>
            {activeCollection && (
              <button onClick={async () => {
                try {
                  for (const id of menu.ids) await api.setCollection(activeCollection, id, false);
                  notify(`已从「${activeCollection}」移出 ${menu.ids.length} 条`); setMenu(null);
                  await Promise.all([refreshMeta(), loadVideos(true)]);
                } catch (e) { notify(String(e)); }
              }}><Icon name="x" size={14} />从当前合集移出</button>
            )}
            <div className="ctx-caption">移入 / 移出合集</div>
            <div className="ctx-colls">
              {activeCollections.filter((c) => c.kind === "custom").map((c) => {
                const first = videoPage.items.find((v) => v.id === menu.ids[0]);
                const has = !!first?.collections.includes(c.name);
                return (
                  <button key={c.id} className={has ? "on" : ""} onClick={async () => {
                    try {
                      for (const id of menu.ids) await api.setCollection(c.id, id, !has);
                      notify(has ? `已从「${c.name}」移出` : `已加入「${c.name}」`); setMenu(null);
                      await Promise.all([refreshMeta(), loadVideos(true)]);
                    } catch (e) { notify(String(e)); }
                  }}>{has ? "✓ " : ""}{c.name}</button>
                );
              })}
              {activeCollections.filter((c) => c.kind === "custom").length === 0 && <span className="muted">还没有手动合集</span>}
            </div>
            <button disabled={menuBusy} onClick={async () => {
              setMenuBusy(true);
              try {
                const path = await api.compressVideos(menu.ids);
                notify(`已打包到 ${path}（原文件保留，点「扫描库」登记压缩包）`);
                setMenu(null);
              } catch (e) { notify(String(e)); } finally { setMenuBusy(false); }
            }}><Icon name="zip" size={14} />{menuBusy ? "压缩中…" : "压缩为 zip（保留原文件）"}</button>
            {view === "trash" ? (
              <button className="danger" onClick={() => void purgeIds(menu.ids)}><Icon name="trash" size={14} />{purgeArmed ? "再点一次确认" : "彻底删除（不可恢复）"}</button>
            ) : (
              <button className="danger" onClick={() => runAction("edit.trash")}><Icon name="trash" size={14} />移入影匣回收站</button>
            )}
          </div>
        </>
      )}

      {renameTarget && (
        <RenameDialog
          video={renameTarget}
          onClose={() => setRenameTarget(null)}
          onDone={(v) => {
            setVideoPage((prev) => ({ ...prev, items: prev.items.map((it) => (it.id === v.id ? v : it)) }));
            void refreshMeta();
          }}
          notify={notify}
        />
      )}

      {moveIds && (
        <MoveDialog
          ids={moveIds}
          currentPath={videoPage.items.find((v) => v.id === moveIds[0])?.path}
          onClose={() => setMoveIds(null)}
          onDone={() => {
            setMoveIds(null);
            setSelected(new Set());
            void loadVideos(true);
            void refreshMeta();
          }}
          notify={notify}
        />
      )}

      {player && (
        <Player
          info={player}
          immersive={immersive}
          onToggleImmersive={(on) => void setImmersiveMode(on)}
          gestures={gestures}
          onClose={() => {
            const id = player?.video.id;
            if (player?.source === "mpv") void api.mpvClose();
            setPlayer(null);
            // 恢复窗口边框，但保留用户的沉浸偏好（下次进来还按偏好走）
            void restoreWindowChrome();
            void refreshMeta();
            if (id) void syncAfterPlay(id); else void loadVideos(true);
          }}
          onNext={() => movePlayer(1)}
          onPrev={() => movePlayer(-1)}
          tags={tags}
          collections={activeCollections}
          onChanged={() => { void refreshMeta(); }}
          onError={notify}
        />
      )}

      {settingsOpen && (
        <SettingsModal settings={settings} setSettings={setSettings} keymap={keymap} setKeymap={setKeymap} gestures={gestures} setGestures={setGestures} onClose={() => setSettingsOpen(false)} notify={notify} />
      )}

      {sheetOpen && (
        <SheetPanel
          status={sheet}
          live={sheetLive}
          onStart={startSheets}
          onCancel={async () => {
            try { await api.cancelContactSheets(); notify("已请求暂停：当前这一条抽完就停，已生成的都保留"); }
            catch (e) { notify(String(e)); }
          }}
          onClear={() => void clearSheets()}
          onClose={() => { setSheetOpen(false); void refreshSheetStatus(); }}
          notify={notify}
        />
      )}

      {collectionsOpen && (
        <CollectionManager
          items={collections.filter((c) => c.kind !== "auto")}
          onChanged={() => { void refreshMeta(); void loadVideos(true); }}
          onClose={() => setCollectionsOpen(false)}
          notify={notify}
        />
      )}

      {dropHover && (
        <div className="drop-veil">
          <div className="drop-card">
            <Icon name="down" size={28} />
            <b>松手即导入</b>
            <span>自动进传输队列，按 0-199 / 200-399… 顺序落盘</span>
          </div>
        </div>
      )}

      {importer && importer.queue_total > 0 && (
        <ImportHud
          status={importer}
          onCancel={() => { void api.cancelImport().catch((e) => notify(String(e))); }}
          onClose={() => setImporter(null)}
        />
      )}

      {bandRect && (
        <div
          className="band-box"
          style={{
            left: Math.min(bandRect.x0, bandRect.x1),
            top: Math.min(bandRect.y0, bandRect.y1),
            width: Math.abs(bandRect.x1 - bandRect.x0),
            height: Math.abs(bandRect.y1 - bandRect.y0),
          }}
        />
      )}

      {toast && <Toast toast={toast} onClose={() => setToast(null)} />}

      {booting && <BootSplash stage={bootStage} fading={bootFade} />}
    </div>
  );
}
