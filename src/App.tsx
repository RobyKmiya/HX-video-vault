import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { Icon } from "./components/Icons";
import { Sidebar, type ViewKey } from "./components/Sidebar";
import { VideoCard } from "./components/VideoCard";
import { DetailDrawer } from "./components/DetailDrawer";
import { Player } from "./components/Player";
import { StatsPanel } from "./components/StatsPanel";
import { TagManager } from "./components/TagManager";
import { api } from "./api";
import { formatBytes } from "./format";
import type {
  Collection,
  LibraryInfo,
  PlaybackInfo,
  Settings,
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
  const [collections, setCollections] = useState<Collection[]>([]);
  const [stats, setStats] = useState<Stats>(EMPTY_STATS);

  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [detailId, setDetailId] = useState<number | null>(null);
  const [player, setPlayer] = useState<PlaybackInfo | null>(null);
  const [statsOpen, setStatsOpen] = useState(false);
  const [tagsOpen, setTagsOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [bulkTagOpen, setBulkTagOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [sheetProgress, setSheetProgress] = useState<{ done: number; total: number } | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const notify = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((t) => (t === msg ? null : t)), 3200);
  }, []);

  const refreshMeta = useCallback(async () => {
    const [t, c, s] = await Promise.all([api.tags(), api.collections(), api.stats()]);
    setTags(t); setCollections(c); setStats(s);
  }, []);

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
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) => {
      return listen("sheet-progress", (e) => {
        const p = e.payload as { done: number; total: number };
        setSheetProgress({ done: p.done, total: p.total });
        if (p.done >= p.total) window.setTimeout(() => setSheetProgress(null), 1200);
      });
    }).then((fn) => {
      if (cancelled) fn(); else unlisten = fn;
    }).catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, []);

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

  const openPlayer = async (video: VideoItem) => {
    try {
      // mpv 可用时优先用它（支持字幕/倍速/进度 IPC 回传）
      const took = await api.mpvOpen(video.id).catch(() => false);
      const result = await api.playback(video.id);
      setPlayer({ ...result, source: took ? "mpv" : result.source });
      await refreshMeta();
    } catch (e) { notify(String(e)); }
  };

  const movePlayer = (delta: number) => {
    const items = videoPage.items;
    const idx = items.findIndex((v) => v.id === player?.video.id);
    const next = items[idx + delta];
    if (!next) { notify(delta > 0 ? "已经是最后一条" : "已经是第一条"); return; }
    void api.playback(next.id).then(setPlayer).catch((e) => notify(String(e)));
  };

  const doScan = async () => {
    if (!info) return;
    setScanning(true);
    try {
      const report = await api.scan({ root: info.root, recursive: true, entry_limit: 60 });
      notify(`扫描完成：新增 ${report.added} 条，更新 ${report.updated} 条，失效 ${report.missing} 条`);
      await Promise.all([loadVideos(true), refreshMeta()]);
    } catch (e) { notify(String(e)); } finally { setScanning(false); }
  };

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

  const showPanel = statsOpen || tagsOpen;

  return (
    <div className="app-shell">
      <Sidebar
        view={view}
        onView={(v) => { setView(v); setActiveTag(null); setActiveCollection(null); setStatsOpen(false); setTagsOpen(false); setSelected(new Set()); }}
        tags={tags}
        activeTag={activeTag}
        onTag={(t) => { setActiveTag(t); setActiveCollection(null); setView("all"); setStatsOpen(false); setTagsOpen(false); }}
        collections={collections}
        activeCollection={activeCollection}
        onCollection={(c) => { setActiveCollection(c); setActiveTag(null); setView("all"); setStatsOpen(false); setTagsOpen(false); }}
        onStats={() => { setStatsOpen(true); setTagsOpen(false); }}
        onTags={() => { setTagsOpen(true); setStatsOpen(false); }}
        onSettings={() => setSettingsOpen(true)}
        statsOpen={statsOpen}
        tagsOpen={tagsOpen}
      />

      <main className="main">
        {showPanel ? (
          statsOpen ? <StatsPanel stats={stats} onClose={() => setStatsOpen(false)} onOpenVideo={(id) => {
            setStatsOpen(false);
            void api.playback(id).then((p) => {
              setPlayer(p);
              void loadVideos(true);
            }).catch((e) => notify(String(e)));
          }} />
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
              <button className="btn" onClick={doScan} disabled={scanning}>
                <Icon name="refresh" size={16} className={scanning ? "spin" : ""} />{scanning ? "扫描中…" : "扫描库"}
              </button>
              <button className="btn" onClick={async () => {
                try {
                  const r = await api.contactSheets(null, 40);
                  notify(`预览图：完成 ${r.done}，跳过 ${r.skipped}，失败 ${r.failed.length}`);
                  await Promise.all([loadVideos(true), refreshMeta()]);
                } catch (e) { notify(String(e)); }
              }} disabled={!!sheetProgress} title="为最近入库、尚无预览图的视频批量抽帧">
                <Icon name="camera" size={16} />
                {sheetProgress ? `${sheetProgress.done}/${sheetProgress.total}` : "批量预览"}
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
                    await api.setCollection("待看", [...selected][0], true);
                    notify("已加入待看清单"); await refreshMeta();
                  } catch (e) { notify(String(e)); }
                }}><Icon name="list" size={14} />加入待看</button>
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
          collections={collections}
          onClose={() => setDetailId(null)}
          onPlay={(v) => void openPlayer(v)}
          onChanged={() => { void refreshMeta(); void loadVideos(true); }}
          onError={notify}
        />
      )}

      {player && (
        <Player
          info={player}
          onClose={() => { if (player?.source === "mpv") void api.mpvClose(); setPlayer(null); void refreshMeta(); void loadVideos(true); }}
          onNext={() => movePlayer(1)}
          onPrev={() => movePlayer(-1)}
          onError={notify}
        />
      )}

      {settingsOpen && (
        <SettingsModal settings={settings} setSettings={setSettings} onClose={() => setSettingsOpen(false)} notify={notify} />
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
