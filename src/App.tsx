import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, DragEvent, MouseEvent as ReactMouseEvent, SetStateAction } from "react";
import { getVersion } from "@tauri-apps/api/app";
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
import { EditPanel } from "./components/EditPanel";
import { MoveDialog } from "./components/MoveDialog";
import { RenameDialog } from "./components/RenameDialog";
import { Splitter } from "./components/Splitter";
import { Toast, type ToastData, type ToastKind } from "./components/Toast";
import { api } from "./api";
import { prewarmCovers, resetCovers } from "./cover";
import { formatBytes } from "./format";
import { usePersistentSize } from "./layout";
import {
  GESTURE_ACTION_LABELS, GESTURE_LABELS, loadGestures, saveGestures,
  type GestureAction, type GestureMap, type GestureSlot,
} from "./gestures";
import {
  ACTIONS, loadKeyMap, saveKeyMap, comboFromEvent, actionForCombo, bindAction, unbindAction,
  type ActionId, type KeyMap,
} from "./shortcuts";
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

/** 导航历史的一帧：主页结构导航（视图/文件夹/标签/收藏夹/排序+方向） */
type NavLoc = { view: ViewKey; folder: string[]; tag: string | null; coll: string | null; sort: string; rev: boolean };

const SORTS = [
  { key: "folder", label: "文件夹" },
  { key: "added", label: "最近加入" },
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
    import_root: null, double_click: "play", immersive_default: false, show_card_info: true,
  });
  const [startupError, setStartupError] = useState<string | null>(null);

  const [view, setView] = useState<ViewKey>("all");
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [sort, setSort] = useState("added");
  // 逆序开关：后端用 "-" 前缀表示取反（无前缀 = 各键默认方向）
  const [sortRev, setSortRev] = useState(false);
  const sortParam = sortRev ? `-${sort}` : sort;
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
      if (player || statsOpen || tagsOpen || downloadOpen || editOpen || detailId || settingsOpen || sheetOpen || collectionsOpen) return;
      if (e.button === 3) navFnRef.current.back(); else navFnRef.current.forward();
    };
    const swallow = (e: MouseEvent) => { if (e.button === 3 || e.button === 4) e.preventDefault(); };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("auxclick", swallow);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("auxclick", swallow);
    };
  }, [player, statsOpen, tagsOpen, downloadOpen, editOpen, detailId, settingsOpen, sheetOpen, collectionsOpen]);

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
    void loadVideos(true);
    void refreshMeta()
      .catch(() => {})
      .finally(() => setBootMetaDone(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, info, view, debouncedSearch, activeTag, activeCollection, sort]);

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
      if (e.key === "Escape") { setDetailId(null); setStatsOpen(false); setTagsOpen(false); setBulkTagOpen(false); }
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

  const selectOne = (video: VideoItem, ev: ReactMouseEvent) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (ev.ctrlKey || ev.metaKey || ev.shiftKey) {
        if (next.has(video.id)) next.delete(video.id); else next.add(video.id);
      } else {
        next.clear(); next.add(video.id);
      }
      return next;
    });
    if (!ev.ctrlKey && !ev.metaKey && !ev.shiftKey) setDetailId(video.id);
  };

  // 库内回收站：只改库标记，磁盘文件不动；二次确认后才执行
  const trashIds = async (ids: number[]) => {
    if (!ids.length) return;
    if (!window.confirm(`把 ${ids.length} 条移入影匣回收站？\n磁盘上的文件不会被删除。`)) return;
    try {
      for (const id of ids) await api.trash(id, true);
      notify(`已移入回收站 ${ids.length} 条（文件原样保留）`);
      setSelected(new Set());
      setDetailId(null);
      setMenu(null);
      await Promise.all([refreshMeta(), loadVideos(true)]);
    } catch (e) { notify(String(e)); }
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
  const calcGridMetrics = useCallback(() => {
    const main = mainRef.current;
    if (!main) return null;
    const chrome = window.innerWidth <= 900 ? 36 : 56;
    const avail = Math.max(208, main.clientWidth - chrome);
    const cols = Math.max(1, Math.floor((avail + 16) / (208 + 16)));
    const cardW = Math.max(208, (avail - (cols - 1) * 16) / cols);
    const rowH = Math.round(cardW * 9 / 16 + (settings.show_card_info === false ? 0 : 78) + 18);
    return { cols, rowH };
  }, [settings.show_card_info]);
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

  const showPanel = statsOpen || tagsOpen || downloadOpen || editOpen;

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
          setStatsOpen(false); setTagsOpen(false); setDownloadOpen(false); setEditOpen(false); setSelected(new Set());
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
        onStats={() => { setStatsOpen(true); setTagsOpen(false); setDownloadOpen(false); setEditOpen(false); }}
        onTags={() => { setTagsOpen(true); setStatsOpen(false); setDownloadOpen(false); setEditOpen(false); }}
        onDownload={() => { setDownloadOpen(true); setStatsOpen(false); setTagsOpen(false); setEditOpen(false); }}
        onEdit={() => { setEditOpen(true); setStatsOpen(false); setTagsOpen(false); setDownloadOpen(false); }}
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
              <button className="btn" onClick={doScan} disabled={scanning} title="增量扫描：已登记且没变过的文件不会再跑 ffprobe">
                <Icon name="refresh" size={16} className={scanning ? "spin" : ""} />{scanLabel}
              </button>
              <button className="btn" onClick={() => { setSheetOpen(true); void refreshSheetStatus(); }} title="批量生成预览图：可暂停、可续跑、可清空重建">
                <Icon name="camera" size={16} className={sheet.running ? "spin" : ""} />
                {sheetLive ? `${sheetLive.done}/${sheetLive.total}` : sheet.pending > 0 ? `预览图 缺 ${sheet.pending}` : "预览图"}
              </button>
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
            <button className="danger" onClick={() => runAction("edit.trash")}><Icon name="trash" size={14} />移入影匣回收站</button>
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

      {toast && <Toast toast={toast} onClose={() => setToast(null)} />}

      {booting && <BootSplash stage={bootStage} fading={bootFade} />}
    </div>
  );
}

/** 启动屏：暗色品牌底 + 脉冲环 logo + 胶片扫光加载条（影=胶片），阶段文案绑真实加载里程碑 */
function BootSplash({ stage, fading }: { stage: number; fading: boolean }) {
  const STAGES = ["打开影匣…", "读取库元数据…", "加载影片墙…", "就绪"];
  const pct = [18, 45, 78, 100][Math.min(stage, 3)];
  return (
    <div className={`boot-splash ${fading ? "fade" : ""}`}>
      <div className="boot-mark">
        <span className="boot-ring r1" />
        <span className="boot-ring r2" />
        <div className="boot-logo"><Icon name="vault" size={34} /></div>
      </div>
      <div className="boot-title">影 匣</div>
      <div className="boot-film">
        <div className="boot-film-strip">
          {Array.from({ length: 8 }, (_, i) => <span key={i} className="boot-frame" />)}
          <span className="boot-sweep" />
        </div>
        <div className="boot-bar"><span style={{ width: `${pct}%` }} /></div>
      </div>
      <div className="boot-stage">
        {STAGES[Math.min(stage, 3)]}
        <span className="boot-dots" />
      </div>
      <div className="boot-foot">本地数据 · 零联网</div>
    </div>
  );
}

/** 角落传输浮窗：队列位置 + 当前文件百分比 + 失败原因；完成后停几秒自退（有失败则留着） */
function ImportHud({
  status, onCancel, onClose,
}: {
  status: ImportStatus;
  onCancel: () => void;
  onClose: () => void;
}) {
  const done = !status.running;
  const filePct = status.current_total > 0
    ? Math.min(100, (status.current_done / status.current_total) * 100)
    : 0;
  const overall = status.queue_total > 0
    ? Math.min(100, (status.queue_done / status.queue_total) * 100)
    : 0;
  const ok = status.queue_done - status.failed;
  return (
    <div className={`import-hud ${done ? "done" : ""}`}>
      <div className="import-hud-head">
        <span className="import-hud-icon">{done ? <Icon name="check" size={14} /> : <Icon name="down" size={14} />}</span>
        <span className="import-hud-title">
          {done
            ? (status.failed > 0 ? `导入结束 · 成功 ${ok} / 失败 ${status.failed}` : `导入完成 · ${ok} 个视频`)
            : `导入中 ${Math.min(status.queue_done + 1, status.queue_total)}/${status.queue_total}`}
        </span>
        <button
          className="hud-x"
          onClick={done ? onClose : onCancel}
          title={done ? "关闭" : "取消导入（当前文件的半成品会清掉，已完成的保留）"}
        >
          <Icon name="x" size={13} />
        </button>
      </div>
      {!done && (
        <>
          <div className="import-hud-file truncate" title={status.current_file}>{status.current_file || "准备中…"}</div>
          <div className="import-hud-bar"><span style={{ width: `${filePct}%` }} /></div>
          <div className="import-hud-meta">
            <span>{formatBytes(status.current_done)} / {formatBytes(status.current_total)}</span>
            <span>整批 {Math.round(overall)}%</span>
          </div>
        </>
      )}
      {status.last_error && (
        <div className="import-hud-err truncate" title={status.last_error}>{status.last_error}</div>
      )}
    </div>
  );
}

function SettingsModal({
  settings, setSettings, keymap, setKeymap, gestures, setGestures, onClose, notify,
}: {
  settings: Settings;
  setSettings: (s: Settings) => void;
  keymap: KeyMap;
  setKeymap: Dispatch<SetStateAction<KeyMap>>;
  gestures: GestureMap;
  setGestures: Dispatch<SetStateAction<GestureMap>>;
  onClose: () => void;
  notify: (m: string) => void;
}) {
  const [draft, setDraft] = useState<Settings>(settings);
  const [recording, setRecording] = useState<ActionId | null>(null);
  /** 录制时的提示（冲突、注册失败…） */
  const [keyHint, setKeyHint] = useState("");
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    // 界面内可自查版本号：更新是否生效一目了然
    getVersion().then(setAppVersion).catch(() => {});
  }, []);
  useEffect(() => {
    // 呼回键的真值在 Rust（注册过的才算数）：打开面板时以 Rust 为准校正一遍，
    // 免得另一处改过之后 localStorage 与实际注册的键对不上。
    void api.getBossRecall()
      .then((binding) => {
        if (!binding) return;
        setKeymap((prev) => {
          if (prev["boss.recall"] === binding) return prev;
          const next = { ...prev, "boss.recall": binding };
          saveKeyMap(next);
          return next;
        });
      })
      .catch(() => {});
  }, [setKeymap]);
  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setRecording(null); setKeyHint(""); return; }
      const combo = comboFromEvent(e);
      if (!combo) return;
      // 冲突提示：同一个组合只能绑一个动作，不覆盖别人的（宁可让人换一个）
      const conflict = (Object.keys(keymap) as ActionId[])
        .find((id) => id !== recording && keymap[id] === combo);
      if (conflict) {
        const label = ACTIONS.find((a) => a.id === conflict)?.label ?? conflict;
        setKeyHint(`「${combo}」已经绑给「${label}」了，换一个`);
        return;
      }
      const previous = keymap;
      const next = bindAction(keymap, recording, combo);
      saveKeyMap(next); setKeymap(next); setRecording(null); setKeyHint("");
      // 呼回键是系统级热键，必须推给 Rust 注册；注册失败就回滚并说明原因
      if (recording === "boss.recall") {
        void api.setBossRecall(combo).catch((error) => {
          setKeyHint(`呼回键「${combo}」注册失败：${String(error)}`);
          saveKeyMap(previous); setKeymap(previous);
        });
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording, keymap, setKeymap]);
  const save = async () => {
    try {
      await api.updateSettings(draft);
      setSettings(draft);
      notify("设置已保存");
      onClose();
    } catch (e) { notify(String(e)); }
  };
  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="panel-head"><div><h1>设置</h1><p>本地配置，不上传{appVersion ? ` · 影匣 v${appVersion}` : ""}</p></div>
          <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button></div>
        <div className="modal-body">
          <div className="field">
            <label>当前库目录</label>
            <div className="field-row">
              <input readOnly value={draft.library_root || "未选择"} />
              <button className="btn" onClick={async () => {
                const dir = await api.pickDirectory();
                if (dir) setDraft((d) => ({ ...d, library_root: dir }));
              }}>选择</button>
            </div>
          </div>
          <div className="field">
            <label>ffmpeg 路径（用于接触表预览）</label>
            <input value={draft.ffmpeg_path || ""} placeholder="留空则自动检测，例如 D:\\MESAVAGE\\Tools\\ffmpeg\\ffmpeg.exe"
              onChange={(e) => setDraft((d) => ({ ...d, ffmpeg_path: e.target.value || null }))} />
          </div>
          <div className="field">
            <label>ffprobe 路径（用于读取时长/编码）</label>
            <input value={draft.ffprobe_path || ""} placeholder="留空则自动检测"
              onChange={(e) => setDraft((d) => ({ ...d, ffprobe_path: e.target.value || null }))} />
          </div>
          <div className="field">
            <label>拖拽导入根目录</label>
            <div className="field-row">
              <input
                value={draft.import_root || ""}
                placeholder={`留空 = 库里自动用 ${draft.library_root ? `${draft.library_root}\\Supervideo` : "<库目录>\\Supervideo"}`}
                onChange={(e) => setDraft((d) => ({ ...d, import_root: e.target.value || null }))}
              />
              <button className="btn" onClick={async () => {
                const dir = await api.pickDirectory();
                if (dir) setDraft((d) => ({ ...d, import_root: dir }));
              }}>选择</button>
              <button className="btn" title="清空 = 用库根下的 Supervideo" onClick={() => setDraft((d) => ({ ...d, import_root: null }))}>默认</button>
            </div>
            <p className="hint">拖视频进来就复制到这个根目录下的区间夹：0-199、200-399…每夹 200 个，顺序排、满了自动开新区间，删出空位就往前补。</p>
          </div>
          <div className="field">
            <label>播放器内核</label>
            <select value={draft.player_pref} onChange={(e) => setDraft((d) => ({ ...d, player_pref: e.target.value }))}>
              <option value="auto">自动（优先 mpv，其次内置）</option>
              <option value="builtin">内置播放器（mp4/webm）</option>
              <option value="mpv">mpv（IPC · 字幕/倍速/截图）</option>
              <option value="vlc">VLC（冷门格式兜底）</option>
            </select>
          </div>
          <div className="field">
            <label>mpv 可执行文件路径（可选）</label>
            <input value={draft.mpv_path || ""} placeholder="留空则自动探测；例如 D:\\TOOLS\\mpv\\mpv.exe"
              onChange={(e) => setDraft((d) => ({ ...d, mpv_path: e.target.value || null }))} />
          </div>
          <div className="field">
            <label>画廊双击</label>
            <select value={draft.double_click || "play"} onChange={(e) => setDraft((d) => ({ ...d, double_click: e.target.value }))}>
              <option value="play">直接播放（推荐）</option>
              <option value="detail">打开详情抽屉</option>
            </select>
            <p className="hint">单击始终是选中并打开详情；双击走这里的设置。</p>
          </div>
          <div className="field setting-checks">
            <label><input type="checkbox" checked={!!draft.immersive_default} onChange={(e) => setDraft((d) => ({ ...d, immersive_default: e.target.checked }))} />播放默认无边框沉浸</label>
            <label><input type="checkbox" checked={draft.show_card_info !== false} onChange={(e) => setDraft((d) => ({ ...d, show_card_info: e.target.checked }))} />卡片显示文件名与信息</label>
          </div>
          <div className="field keymap-field">
            <label>快捷键（默认全部未绑定，点右侧录制）</label>
            <p className="hint">输入框里打字不会触发快捷键；Esc 取消录制。同一个组合只保留最后一个绑定。老板键两个动作是「应用内按键 + 系统级热键」，呼回那个会真的注册到系统。</p>
            <div className="keymap-list">
              {(["排序", "播放", "选择与收藏", "文件", "界面"] as const).map((group) => (
                <div className="keymap-group" key={group}>
                  <div className="keymap-group-title">{group}</div>
                  {ACTIONS.filter((a) => a.group === group).map((a) => (
                    <div className="keymap-row" key={a.id}>
                      <span title={a.hint || a.label}>{a.label}</span>
                      <button className={`keymap-key ${recording === a.id ? "rec" : ""}`} onClick={() => setRecording(a.id)}>
                        {recording === a.id ? "按下组合键…" : keymap[a.id] || "未绑定（点击录制）"}
                      </button>
                      {keymap[a.id] && <button className="keymap-clear" title="清除绑定" onClick={() => {
                        const next = unbindAction(keymap, a.id);
                        saveKeyMap(next); setKeymap(next); setKeyHint("");
                        // 呼回键清掉之后，Rust 侧恢复默认热键（不然就再也唤不回来了）
                        if (a.id === "boss.recall") void api.setBossRecall("Ctrl+Shift+H").catch(() => {});
                      }}><Icon name="x" size={12} /></button>}
                    </div>
                  ))}
                </div>
              ))}
            </div>
            {keyHint && <p className="hint keymap-hint-warn">{keyHint}</p>}
          </div>
          <div className="field keymap-field">
            <label>鼠标手势（播放器内）</label>
            <p className="hint">单击画面仍是立即播放/暂停（加了延迟会发木），所以这里只管双击和 Ctrl+滚轮。</p>
            <div className="keymap-list">
              {(Object.keys(GESTURE_LABELS) as GestureSlot[]).map((slot) => (
                <div className="keymap-row" key={slot}>
                  <span>{GESTURE_LABELS[slot]}</span>
                  <select
                    className="keymap-select"
                    value={gestures[slot]}
                    onChange={(e) => {
                      const next = { ...gestures, [slot]: e.target.value as GestureAction };
                      saveGestures(next); setGestures(next);
                    }}
                  >
                    {(Object.keys(GESTURE_ACTION_LABELS) as GestureAction[]).map((action) => (
                      <option key={action} value={action}>{GESTURE_ACTION_LABELS[action]}</option>
                    ))}
                  </select>
                </div>
              ))}
            </div>
          </div>
          <p className="hint">修改 ffmpeg 路径后需重启影匣生效。影片文件不会被移动或上传。</p>
        </div>
        <div className="modal-actions">
          <button className="btn" onClick={onClose}>取消</button>
          <button className="btn primary" onClick={() => void save()}>保存</button>
        </div>
      </div>
    </div>
  );
}

/** 预览图任务面板：全量生成 / 暂停 / 续跑 / 清空重建 */
function SheetPanel({
  status, live, onStart, onCancel, onClear, onClose, notify,
}: {
  status: SheetStatus;
  live: { done: number; total: number; name?: string } | null;
  onStart: (all: boolean) => Promise<void>;
  onCancel: () => Promise<void>;
  onClear: () => void;
  onClose: () => void;
  notify: (m: string) => void;
}) {
  const [busy, setBusy] = useState<null | "missing" | "all">(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const running = busy !== null || !!live || status.running;
  const total = live?.total ?? status.total;
  const finished = live?.done ?? Math.max(0, status.total - status.pending);
  const pct = total ? Math.min(100, (finished / total) * 100) : 0;

  const run = async (all: boolean) => {
    setBusy(all ? "all" : "missing");
    try { await onStart(all); } finally { setBusy(null); setConfirmClear(false); }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal sheet-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="panel-head">
          <div><h1>预览图</h1><p>ffmpeg 抽 4×4 帧，结果存在库的 .vault/cache 里</p></div>
          <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
        </div>
        <div className="modal-body">
          <div className="sheet-stats">
            <div><b>{status.total.toLocaleString()}</b><span>总视频</span></div>
            <div><b>{status.done.toLocaleString()}</b><span>已有预览图</span></div>
            <div><b>{status.pending.toLocaleString()}</b><span>还缺</span></div>
          </div>

          <div className="sheet-progress">
            <div className="bar"><span style={{ width: `${pct}%` }} /></div>
            <div className="sheet-progress-line">
              <span>{running ? `正在生成 ${finished}/${total}` : `进度 ${finished}/${total}`}</span>
              <span className="muted truncate">{live?.name ?? ""}</span>
            </div>
          </div>

          <div className="sheet-actions">
            <button className="btn primary" disabled={running || status.pending === 0} onClick={() => void run(false)}>
              {busy === "missing" ? "生成中…" : `补齐缺图（${status.pending}）`}
            </button>
            <button className="btn" disabled={running} onClick={() => void run(true)} title="已有的也重抽一遍，慎用">
              全部重做（{status.total}）
            </button>
            <button className="btn" disabled={!running} onClick={() => void onCancel()}>
              暂停
            </button>
            <button
              className="btn danger"
              disabled={running || status.done === 0}
              onClick={() => {
                if (!confirmClear) { setConfirmClear(true); notify("再点一次「确认清空」才会删掉已生成的预览图"); return; }
                setConfirmClear(false);
                onClear();
              }}
            >
              {confirmClear ? "确认清空" : "清空重建"}
            </button>
          </div>

          <p className="hint">
            可以随时关掉这个窗口，甚至退出影匣：每条抽完立刻入库，已完成的不丢。
            下次进来点「补齐缺图」就从剩下的继续。清空重建只删预览图，不动视频文件。
          </p>
        </div>
      </div>
    </div>
  );
}

/** 目录管理：新建 / 改名 / 分组 / 软删除 / 恢复 / 彻底删除 */
function CollectionManager({
  items, onChanged, onClose, notify,
}: {
  items: Collection[];
  onChanged: () => void;
  onClose: () => void;
  notify: (m: string) => void;
}) {
  const [tab, setTab] = useState<"active" | "trash">("active");
  const [newName, setNewName] = useState("");
  const [newGroup, setNewGroup] = useState("");
  const [editing, setEditing] = useState<number | null>(null);
  const [draftName, setDraftName] = useState("");
  const [draftGroup, setDraftGroup] = useState("");
  const [confirmPurge, setConfirmPurge] = useState<number | null>(null);

  const active = items.filter((c) => c.deleted_at === 0);
  const trashed = items.filter((c) => c.deleted_at > 0);
  const groups = [...new Set(active.map((c) => c.group_name).filter(Boolean))];

  const create = async () => {
    if (!newName.trim()) { notify("先给收藏夹起个名字"); return; }
    try {
      await api.createCollection(newName, newGroup);
      setNewName("");
      onChanged();
      notify(`收藏夹「${newName.trim()}」已就绪`);
    } catch (e) { notify(String(e)); }
  };

  const saveEdit = async (id: number) => {
    try {
      await api.renameCollection(id, draftName);
      await api.setCollectionGroup(id, draftGroup);
      setEditing(null);
      onChanged();
      notify("已保存");
    } catch (e) { notify(String(e)); }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal collection-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="panel-head">
          <div><h1>收藏夹管理</h1><p>自建收藏夹：可改名、可分组、可拖拽排序（侧栏）、删除进回收站</p></div>
          <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
        </div>

        <div className="modal-tabs">
          <button className={tab === "active" ? "on" : ""} onClick={() => setTab("active")}>收藏夹（{active.length}）</button>
          <button className={tab === "trash" ? "on" : ""} onClick={() => setTab("trash")}>回收站（{trashed.length}）</button>
        </div>

        <div className="modal-body">
          {tab === "active" ? (
            <>
              <div className="collection-new" title="新建后可在侧栏拖拽排序">
                <input value={newName} placeholder="新目录名" onChange={(e) => setNewName(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") void create(); }} />
                <input value={newGroup} placeholder="分组（可空）" list="vv-collection-groups"
                  onChange={(e) => setNewGroup(e.target.value)} />
                <datalist id="vv-collection-groups">
                  {groups.map((g) => <option key={g} value={g} />)}
                </datalist>
                <button className="btn primary" onClick={() => void create()}>新建</button>
              </div>

              <div className="collection-list">
                {active.map((c) => {
                  const isSystem = c.kind === "system" || c.kind === "auto";
                  const isEditing = editing === c.id;
                  return (
                    <div key={c.id} className="collection-row">
                      {isEditing ? (
                        <>
                          <input className="collection-input" value={draftName} onChange={(e) => setDraftName(e.target.value)} autoFocus />
                          <input className="collection-input" value={draftGroup} placeholder="分组" list="vv-collection-groups"
                            onChange={(e) => setDraftGroup(e.target.value)} />
                          <button className="btn small primary" onClick={() => void saveEdit(c.id)}>保存</button>
                          <button className="btn small" onClick={() => setEditing(null)}>取消</button>
                        </>
                      ) : (
                        <>
                          <span className="collection-name">
                            <Icon name="folder" size={15} />
                            <span className="truncate">{c.name}</span>
                            {c.group_name && <span className="group-chip">{c.group_name}</span>}
                            {c.kind === "auto" && <span className="group-chip">跟随文件夹</span>}
                            {c.kind === "system" && <span className="muted small">内置</span>}
                          </span>
                          <span className="nav-count">{c.count}</span>
                          <button className="btn small" disabled={isSystem}
                            onClick={() => { setEditing(c.id); setDraftName(c.name); setDraftGroup(c.group_name); }}>
                            改名 / 分组
                          </button>
                          <button className="btn small danger" disabled={isSystem}
                            onClick={async () => {
                              try { await api.deleteCollection(c.id); onChanged(); notify(`「${c.name}」已移入回收站`); }
                              catch (e) { notify(String(e)); }
                            }}>
                            删除
                          </button>
                        </>
                      )}
                    </div>
                  );
                })}
                {active.length === 0 && <div className="nav-empty">还没有收藏夹</div>}
              </div>
            </>
          ) : (
            <div className="collection-list">
              {trashed.map((c) => (
                <div key={c.id} className="collection-row">
                  <span className="collection-name">
                    <Icon name="trash" size={15} />
                    <span className="truncate">{c.name}</span>
                    {c.group_name && <span className="group-chip">{c.group_name}</span>}
                    <span className="muted small">{c.count} 条</span>
                  </span>
                  <button className="btn small" onClick={async () => {
                    try { await api.restoreCollection(c.id); onChanged(); notify(`「${c.name}」已恢复`); }
                    catch (e) { notify(String(e)); }
                  }}>恢复</button>
                  <button className={`btn small ${confirmPurge === c.id ? "danger" : ""}`}
                    onClick={async () => {
                      if (confirmPurge !== c.id) { setConfirmPurge(c.id); notify("再点一次才会彻底删除目录（视频本身不受影响）"); return; }
                      setConfirmPurge(null);
                      try { await api.deleteCollection(c.id, true); onChanged(); notify(`「${c.name}」已彻底删除`); }
                      catch (e) { notify(String(e)); }
                    }}>
                    {confirmPurge === c.id ? "确认彻底删除" : "彻底删除"}
                  </button>
                </div>
              ))}
              {trashed.length === 0 && <div className="nav-empty">回收站是空的</div>}
            </div>
          )}

          <p className="hint">删除收藏夹只是把它放进回收站，视频文件和在库里的记录都不受影响。磁盘文件夹由扫描自动镜像（主页层级浏览可见），是存储结构，不在这里列出。</p>
        </div>
      </div>
    </div>
  );
}
