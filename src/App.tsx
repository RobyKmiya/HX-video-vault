import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { Icon } from "./components/Icons";
import { Sidebar, type ViewKey } from "./components/Sidebar";
import { VideoCard } from "./components/VideoCard";
import { DetailDrawer } from "./components/DetailDrawer";
import { Player } from "./components/Player";
import { StatsPanel } from "./components/StatsPanel";
import { TagManager } from "./components/TagManager";
import { DownloadPanel } from "./components/DownloadPanel";
import { api } from "./api";
import { formatBytes } from "./format";
import type {
  Collection,
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

const SORTS = [
  { key: "added", label: "最近加入" },
  { key: "watched", label: "最近观看" },
  { key: "title", label: "文件名" },
  { key: "size", label: "大小" },
  { key: "duration", label: "时长" },
  { key: "count", label: "观看次数" },
  { key: "rating", label: "评分" },
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
  });
  const [startupError, setStartupError] = useState<string | null>(null);

  const [view, setView] = useState<ViewKey>("all");
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [sort, setSort] = useState("added");
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
  const [scanning, setScanning] = useState(false);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [bulkTagOpen, setBulkTagOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [sheet, setSheet] = useState<SheetStatus>({ running: false, total: 0, done: 0, pending: 0 });
  const [sheetLive, setSheetLive] = useState<{ done: number; total: number; name?: string } | null>(null);
  const [collectionsOpen, setCollectionsOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const reloadRef = useRef<() => void>(() => {});
  const lastScanRefresh = useRef(0);

  const notify = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((t) => (t === msg ? null : t)), 3200);
  }, []);

  const refreshMeta = useCallback(async () => {
    const [t, c, s] = await Promise.all([api.tags(), api.collections(true), api.stats()]);
    setTags(t); setCollections(c); setStats(s);
  }, []);

  const refreshSheetStatus = useCallback(async () => {
    try { setSheet(await api.sheetStatus()); } catch { /* 库未打开时忽略 */ }
  }, []);

  const activeCollections = useMemo(() => collections.filter((c) => c.deleted_at === 0), [collections]);

  const loadVideos = useCallback(async (reset = true) => {
    setLoading(true);
    try {
      const nextPage = reset ? 1 : page + 1;
      const result = await api.query({
        view, search: debouncedSearch || undefined, tag: activeTag ?? undefined,
        collection: activeCollection ?? undefined, sort, page: nextPage, page_size: 48,
      });
      setPage(nextPage);
      setVideoPage((prev) => reset ? result : {
        ...result, items: [...prev.items, ...result.items],
      });
    } catch (e) {
      notify(String(e));
    } finally {
      setLoading(false);
    }
  }, [view, debouncedSearch, activeTag, activeCollection, sort, page, notify]);

  useEffect(() => {
    api.settings().then(([s, err, i]) => {
      setSettings(s); setInfo(i); setStartupError(err); setReady(true);
    }).catch((e) => { notify(String(e)); setReady(true); });
  }, [notify]);

  useEffect(() => {
    const t = window.setTimeout(() => setDebouncedSearch(search), 260);
    return () => window.clearTimeout(t);
  }, [search]);

  useEffect(() => {
    if (!ready || !info) return;
    void loadVideos(true);
    void refreshMeta();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, info, view, debouncedSearch, activeTag, activeCollection, sort]);

  useEffect(() => {
    reloadRef.current = () => void loadVideos(true);
  }, [loadVideos]);

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

  const detail = useMemo(
    () => videoPage.items.find((v) => v.id === detailId) ?? null,
    [videoPage.items, detailId],
  );

  // 打开播放统一走这里：先试 mpv（支持字幕/倍速/IPC），拿不到就退回内置。
  // 首次打开与「上一条/下一条」共用同一条路径，避免翻页时上一集的 mpv 窗口留在屏幕上。
  const openById = useCallback(async (id: number) => {
    try {
      const took = await api.mpvOpen(id).catch(() => false);
      if (!took) void api.mpvClose().catch(() => {});
      const result = await api.playback(id);
      setPlayer({ ...result, source: took ? "mpv" : result.source });
      // 正在播的那条就是「当前选中的那条」，关掉播放器后网格要知道该高亮谁
      setSelected(new Set([id]));
      setDetailId((cur) => (cur === null ? null : id));
      await refreshMeta();
    } catch (e) { notify(String(e)); }
  }, [notify, refreshMeta]);

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
          collection: activeCollection ?? undefined, sort, page: videoPage.page + 1, page_size: 48,
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
        `扫描完成：新增 ${report.added} 条，更新 ${report.updated} 条，失效 ${report.missing} 条` +
        `（本次识别 ${report.probed ?? 0} 条，跳过 ${report.reused ?? 0} 条）`,
      );
      await Promise.all([loadVideos(true), refreshMeta()]);
    } catch (e) { notify(String(e)); } finally { setScanning(false); setScanProgress(null); }
  };

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

  const scanLabel = !scanning
    ? "扫描库"
    : scanProgress
      ? `${SCAN_PHASE[scanProgress.phase]} ${scanProgress.done}/${scanProgress.total}`
      : "扫描中…";

  const selectOne = (video: VideoItem, ev: MouseEvent) => {
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

  if (!ready) {
    return <div className="boot"><Icon name="vault" size={38} /><span>正在打开影匣…</span></div>;
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
        {settingsOpen && <SettingsModal settings={settings} setSettings={setSettings} onClose={() => setSettingsOpen(false)} notify={notify} />}
      </div>
    );
  }

  const showPanel = statsOpen || tagsOpen || downloadOpen;

  return (
    <div className="app-shell">
      <Sidebar
        view={view}
        onView={(v) => { setView(v); setActiveTag(null); setActiveCollection(null); setStatsOpen(false); setTagsOpen(false); setDownloadOpen(false); setSelected(new Set()); }}
        tags={tags}
        activeTag={activeTag}
        onTag={(t) => { setActiveTag(t); setActiveCollection(null); setView("all"); setStatsOpen(false); setTagsOpen(false); }}
        collections={activeCollections}
        activeCollection={activeCollection}
        onCollection={(c) => { setActiveCollection(c); setActiveTag(null); setView("all"); setStatsOpen(false); setTagsOpen(false); }}
        onCreateCollection={() => setCollectionsOpen(true)}
        onManageCollections={() => setCollectionsOpen(true)}
        onStats={() => { setStatsOpen(true); setTagsOpen(false); setDownloadOpen(false); }}
        onTags={() => { setTagsOpen(true); setStatsOpen(false); setDownloadOpen(false); }}
        onDownload={() => { setDownloadOpen(true); setStatsOpen(false); setTagsOpen(false); }}
        onSettings={() => setSettingsOpen(true)}
        statsOpen={statsOpen}
        tagsOpen={tagsOpen}
        downloadOpen={downloadOpen}
      />

      <main className="main">
        {showPanel ? (
          statsOpen ? <StatsPanel stats={stats} onClose={() => setStatsOpen(false)} onOpenVideo={(id) => {
            setStatsOpen(false);
            void openById(id);
          }} />
            : downloadOpen ? <DownloadPanel tags={tags} onClose={() => setDownloadOpen(false)}
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
              <select className="select" value={sort} onChange={(e) => setSort(e.target.value)} title="排序">
                {SORTS.map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
              </select>
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
                  <span className="filter-chip">{activeTag ? `标签：${activeTag}` : `目录：${activeCollection}`}
                    <button onClick={() => { setActiveTag(null); setActiveCollection(null); }}><Icon name="x" size={12} /></button>
                  </span>
                )}
              </div>
              <div className="result-right">
                <span className="muted">库：{info.root}</span>
                <span className="size-chip">{formatBytes(stats.total_bytes)}</span>
              </div>
            </div>

            {selected.size > 0 && (
              <div className="bulk-bar">
                <span>已选 <b>{selected.size}</b> 条</span>
                <button className="btn small" onClick={() => setBulkTagOpen((v) => !v)}><Icon name="tag" size={14} />批量打标</button>
                <button className="btn small" onClick={async () => {
                  try {
                    const ids = [...selected];
                    for (const id of ids) await api.setCollection("待看", id, true);
                    notify(`已把 ${ids.length} 条加入待看`); await refreshMeta();
                  } catch (e) { notify(String(e)); }
                }}><Icon name="list" size={14} />加入待看</button>
                <select className="select small-select" value="" onChange={async (e) => {
                  const id = Number(e.target.value);
                  if (!id) return;
                  try {
                    const ids = [...selected];
                    for (const v of ids) await api.setCollection(id, v, true);
                    notify(`已把 ${ids.length} 条加入该目录`);
                    await Promise.all([refreshMeta(), loadVideos(true)]);
                  } catch (err) { notify(String(err)); }
                  e.target.value = "";
                }} title="批量加入目录">
                  <option value="">加入目录…</option>
                  {activeCollections.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
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

            {videoPage.items.length === 0 ? (
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
                <div className="video-grid">
                  {videoPage.items.map((v) => (
                    <VideoCard
                      key={`${v.id}`}
                      video={v}
                      selected={selected.has(v.id)}
                      onSelect={selectOne}
                      onOpen={(item) => setDetailId(item.id)}
                      onPlay={(item) => void openPlayer(item)}
                    />
                  ))}
                </div>
                {videoPage.items.length < videoPage.total && (
                  <div className="load-more">
                    <button className="btn" onClick={() => void loadVideos(false)} disabled={loading}>
                      {loading ? "加载中…" : `加载更多（还有 ${videoPage.total - videoPage.items.length} 条）`}
                    </button>
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
          onChanged={() => { void refreshMeta(); void loadVideos(true); }}
          onError={notify}
        />
      )}

      {player && (
        <Player
          info={player}
          onClose={() => {
            const id = player?.video.id;
            if (player?.source === "mpv") void api.mpvClose();
            setPlayer(null);
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
        <SettingsModal settings={settings} setSettings={setSettings} onClose={() => setSettingsOpen(false)} notify={notify} />
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
          items={collections}
          onChanged={() => { void refreshMeta(); void loadVideos(true); }}
          onClose={() => setCollectionsOpen(false)}
          notify={notify}
        />
      )}

      {toast && <div className="toast"><Icon name="info" size={15} />{toast}</div>}
    </div>
  );
}

function SettingsModal({
  settings, setSettings, onClose, notify,
}: {
  settings: Settings;
  setSettings: (s: Settings) => void;
  onClose: () => void;
  notify: (m: string) => void;
}) {
  const [draft, setDraft] = useState<Settings>(settings);
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
        <div className="panel-head"><div><h1>设置</h1><p>本地配置，不上传</p></div>
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
    if (!newName.trim()) { notify("先给目录起个名字"); return; }
    try {
      await api.createCollection(newName, newGroup);
      setNewName("");
      onChanged();
      notify(`目录「${newName.trim()}」已就绪`);
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
          <div><h1>目录管理</h1><p>自定义收藏夹：可改名、可分组、删除进回收站</p></div>
          <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
        </div>

        <div className="modal-tabs">
          <button className={tab === "active" ? "on" : ""} onClick={() => setTab("active")}>目录（{active.length}）</button>
          <button className={tab === "trash" ? "on" : ""} onClick={() => setTab("trash")}>回收站（{trashed.length}）</button>
        </div>

        <div className="modal-body">
          {tab === "active" ? (
            <>
              <div className="collection-new">
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
                  const isSystem = c.kind === "system";
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
                            {isSystem && <span className="muted small">内置</span>}
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
                {active.length === 0 && <div className="nav-empty">还没有自定义目录</div>}
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

          <p className="hint">删除目录只是把目录放进回收站，视频文件和在库里的记录都不受影响。</p>
        </div>
      </div>
    </div>
  );
}
