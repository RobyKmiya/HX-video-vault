import { useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import { formatDuration } from "../format";
import type { EditStatus, VideoItem } from "../types";

/**
 * 剪辑面板：素材可选库内条目或外来文件。
 * 单素材 = 裁剪（默认流复制秒出，可选精确重编码）；多素材 = 按顺序拼接（参数一致秒拼，不一致自动重编码对齐第一段）。
 * 成品写入库根目录的「剪辑输出」文件夹，完成后自动增量扫描入库。
 */

interface EditItem {
  key: string;
  id?: number;
  path?: string;
  name: string;
  duration: number;
}

interface Props {
  /** 从详情页 / 批量栏带进来的预填素材，只在变化时应用 */
  prefill: VideoItem[] | null;
  onClose: () => void;
  onImported: () => void;
  notify: (m: string) => void;
}

/** "1:23" / "1:02:03" / "83.5" → 秒；解析失败返回 NaN */
function parseTime(text: string, fallback: number): number {
  const t = text.trim();
  if (!t) return fallback;
  if (/^\d+(\.\d+)?$/.test(t)) return Number(t);
  const parts = t.split(":").map(Number);
  if (parts.some((n) => !Number.isFinite(n))) return NaN;
  return parts.reduce((acc, n) => acc * 60 + n, 0);
}

function fmtTime(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) return "0:00";
  const s = Math.floor(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
  return `${m}:${String(ss).padStart(2, "0")}`;
}

export function EditPanel({ prefill, onClose, onImported, notify }: Props) {
  const [items, setItems] = useState<EditItem[]>([]);
  const [startText, setStartText] = useState("0:00");
  const [endText, setEndText] = useState("");
  const [accurate, setAccurate] = useState(false);
  const [search, setSearch] = useState("");
  const [results, setResults] = useState<VideoItem[]>([]);
  const [job, setJob] = useState<EditStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const prefillKey = useRef(0);

  // 从详情页/批量栏带进来的素材
  useEffect(() => {
    if (!prefill || prefill.length === 0) return;
    prefillKey.current += 1;
    const mapped: EditItem[] = prefill.map((v) => ({
      key: `id-${v.id}-${prefillKey.current}`,
      id: v.id,
      name: v.file_name,
      duration: v.duration_sec,
    }));
    setItems(mapped);
    const dur = mapped[0]?.duration ?? 0;
    setStartText("0:00");
    setEndText(dur > 0 ? fmtTime(dur) : "");
  }, [prefill]);

  // 关掉面板再进来能恢复正在跑的任务现场
  useEffect(() => {
    api.editStatus().then((s) => { if (s.running) setJob(s); }).catch(() => {});
  }, []);

  // 进度事件只管展示；成功/失败的结果以 invoke 的返回/异常为准
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) => {
      return listen("edit-progress", (e) => {
        const p = e.payload as EditStatus;
        setJob(p.running ? p : null);
      });
    }).then((fn) => {
      if (cancelled) fn(); else unlisten = fn;
    }).catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  // 库内搜索（防抖）
  useEffect(() => {
    const t = window.setTimeout(() => {
      const q = search.trim();
      if (!q) { setResults([]); return; }
      api.query({ search: q, page_size: 8 }).then((p) => setResults(p.items)).catch(() => {});
    }, 300);
    return () => window.clearTimeout(t);
  }, [search]);

  const mode = items.length === 1 ? "trim" : items.length >= 2 ? "concat" : "empty";
  const running = busy || (job?.running ?? false);

  const addLibraryItem = (v: VideoItem) => {
    if (items.some((it) => it.id === v.id)) { notify("这条已经在素材列表里"); return; }
    setItems((s) => [...s, { key: `id-${v.id}-${Date.now()}`, id: v.id, name: v.file_name, duration: v.duration_sec }]);
    setSearch(""); setResults([]);
  };

  const addFiles = async () => {
    try {
      const paths = await api.pickVideoFiles();
      if (paths.length === 0) return;
      for (const p of paths) {
        if (items.some((it) => it.path === p)) continue;
        try {
          const meta = await api.probeMedia(p);
          const name = p.split(/[\\/]/).pop() || p;
          setItems((s) => [...s, { key: `p-${p}-${Date.now()}`, path: p, name, duration: meta.duration_sec }]);
        } catch {
          notify(`跳过（读不出时长）：${p.split(/[\\/]/).pop()}`);
        }
      }
    } catch (e) { notify(String(e)); }
  };

  const removeItem = (key: string) => setItems((s) => s.filter((it) => it.key !== key));
  const moveItem = (key: string, delta: number) => setItems((s) => {
    const idx = s.findIndex((it) => it.key === key);
    const next = idx + delta;
    if (idx < 0 || next < 0 || next >= s.length) return s;
    const copy = [...s];
    [copy[idx], copy[next]] = [copy[next], copy[idx]];
    return copy;
  });

  const totalDuration = useMemo(() => items.reduce((acc, it) => acc + it.duration, 0), [items]);

  const afterDone = async () => {
    // 触发一次增量扫描让成品入库（.part 改名后文件已就绪）
    try { await api.scan({ root: "", recursive: true }); } catch { /* 扫描失败不打断 */ }
    onImported();
  };

  const start = async () => {
    if (items.length === 0) { notify("先添加至少一个素材"); return; }
    if (mode === "trim") {
      const it = items[0];
      const startSec = parseTime(startText, 0);
      const endSec = parseTime(endText, it.duration);
      if (!Number.isFinite(startSec) || !Number.isFinite(endSec) || endSec <= startSec) {
        notify("起止时间无效：终点必须大于起点（支持 0:12 / 1:02:03 / 秒数）");
        return;
      }
      if (it.duration > 0 && endSec > it.duration + 0.5) {
        notify(`终点超出视频时长（${fmtTime(it.duration)}）`);
        return;
      }
      setBusy(true);
      try {
        const r = await api.trimVideo(it.id != null ? { id: it.id } : { path: it.path }, startSec, endSec, accurate);
        notify(`裁剪完成：${r.output_path}${r.note ? `（${r.note}）` : ""}`);
        await afterDone();
      } catch (e) { notify(String(e)); } finally { setBusy(false); setJob(null); }
    } else {
      setBusy(true);
      try {
        const r = await api.concatVideo(items.map((it) => (it.id != null ? { id: it.id } : { path: it.path })));
        notify(`拼接完成：${r.output_path}${r.note ? `（${r.note}）` : ""}`);
        await afterDone();
      } catch (e) { notify(String(e)); } finally { setBusy(false); setJob(null); }
    }
  };

  const cancel = async () => {
    try { await api.cancelEdit(); notify("已请求取消：ffmpeg 会立刻停掉，半成品自动清理"); }
    catch (e) { notify(String(e)); }
  };

  const percent = job?.percent ?? 0;

  return (
    <section className="stats-panel">
      <div className="panel-head">
        <div><h1>剪辑</h1><p>裁剪与拼接 · 素材可选库内条目或外来文件 · 成品自动入库</p></div>
        <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
      </div>

      <div className="panel-card">
        <div className="resolve-head"><h2>素材（{items.length}）</h2>
          {mode === "trim" && <span className="muted">裁剪模式</span>}
          {mode === "concat" && <span className="muted">拼接模式 · 按列表顺序</span>}
        </div>
        {items.length === 0 ? (
          <p className="muted">从下面搜索库内条目，或添加外来文件。一条=裁剪，多条=拼接。</p>
        ) : (
          <div className="edit-list">
            {items.map((it, i) => (
              <div className="edit-item" key={it.key}>
                <span className="edit-index">{i + 1}</span>
                <div className="edit-item-main">
                  <div className="truncate" title={it.path || `库内 #${it.id}`}>{it.name}</div>
                  <div className="muted">
                    {it.duration > 0 ? formatDuration(it.duration) : "时长待探测"}
                    {it.id == null && <span className="edit-tag">外来文件</span>}
                    {it.id != null && it.duration <= 0 && <span className="edit-tag">压缩包条目</span>}
                  </div>
                </div>
                {mode === "concat" && (
                  <span className="edit-order">
                    <button className="icon-btn" title="上移" onClick={() => moveItem(it.key, -1)}><Icon name="left" size={14} /></button>
                    <button className="icon-btn" title="下移" onClick={() => moveItem(it.key, 1)}><Icon name="right" size={14} /></button>
                  </span>
                )}
                <button className="icon-btn" title="移除" onClick={() => removeItem(it.key)}><Icon name="x" size={15} /></button>
              </div>
            ))}
          </div>
        )}
        {mode === "concat" && items.length > 1 && (
          <p className="hint">总时长约 {formatDuration(totalDuration)}。各段编码/分辨率一致时秒拼（流复制）；不一致会自动重新编码对齐第一段（较慢）。</p>
        )}
      </div>

      {mode === "trim" && (
        <div className="panel-card">
          <h2>裁剪范围</h2>
          <div className="edit-times">
            <label>起点<input value={startText} onChange={(e) => setStartText(e.target.value)} placeholder="0:00" /></label>
            <label>终点<input value={endText} onChange={(e) => setEndText(e.target.value)} placeholder={items[0]?.duration > 0 ? fmtTime(items[0].duration) : "0:30"} /></label>
            <button className="btn small" onClick={() => setEndText(fmtTime(items[0]?.duration || 0))}>到结尾</button>
          </div>
          <label className="edit-accurate">
            <input type="checkbox" checked={accurate} onChange={(e) => setAccurate(e.target.checked)} />
            精确到帧（重新编码，慢；不勾=流复制，瞬间完成但切割点对齐关键帧，可能有几秒偏差）
          </label>
        </div>
      )}

      <div className="panel-card">
        <h2>添加素材</h2>
        <div className="download-input">
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜库内视频文件名…"
          />
          <button className="btn" onClick={() => void addFiles()}>
            <Icon name="plus" size={15} />外来文件
          </button>
        </div>
        {results.length > 0 && (
          <div className="edit-picker">
            {results.map((v) => (
              <button className="edit-picker-row" key={v.id} onClick={() => addLibraryItem(v)} title={v.path}>
                <Icon name="plus" size={13} />
                <span className="truncate">{v.file_name}</span>
                <span className="muted">{v.duration_sec > 0 ? formatDuration(v.duration_sec) : ""}</span>
              </button>
            ))}
          </div>
        )}
        <p className="hint">压缩包里的视频也能当素材（会自动解到缓存再剪）；外来文件不会被移动或修改。</p>
      </div>

      {running ? (
        <div className="panel-card">
          <div className="resolve-head"><h2>正在剪辑</h2>
            <button className="btn small danger" onClick={() => void cancel()}>取消</button>
          </div>
          <div className="task-bar"><span style={{ width: `${Math.max(0, Math.min(100, percent))}%` }} /></div>
          <div className="task-meta muted">
            {job?.phase === "probe" ? "解析素材…" : `${percent.toFixed(0)}%`} · {job?.message || ""}
          </div>
        </div>
      ) : (
        <div className="panel-card">
          <div className="edit-actions">
            <button className="btn primary" disabled={items.length === 0} onClick={() => void start()}>
              <Icon name={mode === "concat" ? "merge" : "cut"} size={15} />
              {mode === "concat" ? `拼接 ${items.length} 段` : "开始裁剪"}
            </button>
            <span className="muted">成品 → 库目录下的「剪辑输出」文件夹，完成后自动扫描入库</span>
          </div>
        </div>
      )}
    </section>
  );
}
