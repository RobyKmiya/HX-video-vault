import { useCallback, useEffect, useMemo, useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import { formatBytes } from "../format";
import type { DuplicateGroup, FingerprintProgress } from "../types";

interface Props {
  onClose: () => void;
  /** 清理完通知主页刷新列表 */
  onChanged: () => void;
  notify: (m: string) => void;
}

/**
 * 重复清理面板。
 *
 * 只标记、不删文件：命中的副本统一进回收站，用户在回收站里确认后再删。
 * 这样万一把「只改了中间片段」的剪辑变体误判成重复，还有挽回的余地。
 */
export function DedupePanel({ onClose, onChanged, notify }: Props) {
  const [status, setStatus] = useState<FingerprintProgress>({ done: 0, total: 0, running: false });
  const [groups, setGroups] = useState<DuplicateGroup[]>([]);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(false);

  const loadGroups = useCallback(async () => {
    const g = await api.dedupeGroups();
    setGroups(g);
    // 默认勾上「建议清掉」的副本，用户可以逐条取消
    setPicked(new Set(g.flatMap((x) => x.others.map((o) => o.id))));
  }, []);

  const refresh = useCallback(async () => {
    const s = await api.dedupeStatus();
    setStatus(s);
    if (!s.running && s.total > 0 && s.done >= s.total) {
      await loadGroups();
    }
    setLoaded(true);
  }, [loadGroups]);

  useEffect(() => {
    let dead = false;
    void refresh().catch((e) => notify(String(e)));
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(({ listen }) => listen("dedupe-progress", () => {
        if (!dead) void refresh().catch(() => {});
      }))
      .then((fn) => { if (dead) fn(); else unlisten = fn; })
      .catch(() => {});
    return () => { dead = true; unlisten?.(); };
  }, [refresh, notify]);

  const selected = useMemo(
    () => groups
      .map((g) => ({ keep: g.keep.id, drops: g.others.filter((o) => picked.has(o.id)).map((o) => o.id) }))
      .filter((p) => p.drops.length > 0),
    [groups, picked],
  );

  const waste = useMemo(() => {
    let sum = 0;
    for (const g of groups) for (const o of g.others) if (picked.has(o.id)) sum += o.size_bytes;
    return sum;
  }, [groups, picked]);

  const startScan = async () => {
    try {
      await api.dedupeScan();
      setStatus((s) => ({ ...s, running: true }));
    } catch (e) { notify(String(e)); }
  };

  const apply = async () => {
    if (!selected.length) { notify("没有勾选要清理的副本"); return; }
    setBusy(true);
    try {
      const n = await api.dedupeApplySelected(selected.map((p) => [p.keep, p.drops] as [number, number[]]));
      notify(`已把 ${n} 个重复副本送进回收站；到回收站确认后再删文件，可腾出约 ${formatBytes(waste)}`);
      await loadGroups();
      onChanged();
    } catch (e) { notify(String(e)); }
    finally { setBusy(false); }
  };

  const toggle = (id: number) => setPicked((s) => {
    const n = new Set(s);
    if (n.has(id)) n.delete(id); else n.add(id);
    return n;
  });

  const scanned = status.total > 0 && status.done >= status.total && !status.running;
  const pct = status.total > 0 ? Math.round((status.done / status.total) * 100) : 0;
  const totalWaste = groups.reduce((a, g) => a + g.wasted_bytes, 0);

  return (
    <section className="stats-panel">
      <div className="panel-head">
        <div>
          <h1>重复清理</h1>
          <p>按内容指纹（大小 + 首尾各 8MB）找同一份视频的多份副本；只标记进回收站，不直接删文件</p>
        </div>
        <button className="icon-btn" onClick={onClose} title="关闭"><Icon name="x" size={18} /></button>
      </div>

      <div className="panel-card">
        <div className="resolve-head">
          <div>
            <h2>指纹进度</h2>
            <p className="muted">
              {status.running
                ? `正在算… ${status.done} / ${status.total}（${pct}%）`
                : status.total === 0
                  ? "库里还没有可查重的视频"
                  : scanned
                    ? `已算完 ${status.done} 条，可以看结果了`
                    : `已算 ${status.done} / ${status.total}`}
            </p>
          </div>
          <button className="btn" onClick={() => void startScan()} disabled={status.running || !loaded}>
            <Icon name="refresh" size={14} className={status.running ? "spin" : ""} />
            {status.running ? "计算中…" : status.done > 0 ? "继续算" : "开始查重"}
          </button>
        </div>
        <div className="task-bar" style={{ marginTop: 10 }}><span style={{ width: `${pct}%` }} /></div>
        <p className="hint">
          大文件排在前面先算——它们才是空间大头。跳过 mtime，所以跨盘复制出来的副本也能认出来。
        </p>
      </div>

      {scanned && (
        <div className="panel-card">
          <div className="resolve-head">
            <div>
              <h2>
                {groups.length > 0
                  ? `发现 ${groups.length} 组重复 · 共 ${formatBytes(totalWaste)}`
                  : "没有发现内容重复的视频"}
              </h2>
              <p className="muted">保留项是看过次数最多、或看得最近的那份，看片进度不会丢</p>
            </div>
            {groups.length > 0 && (
              <button className="btn primary" onClick={() => void apply()} disabled={busy || !selected.length}>
                <Icon name="trash" size={14} />
                {busy ? "处理中…" : `清理 ${selected.reduce((a, p) => a + p.drops.length, 0)} 个 · ${formatBytes(waste)}`}
              </button>
            )}
          </div>

          <div className="dedupe-list">
            {groups.map((g) => (
              <div className="dedupe-group" key={g.fingerprint}>
                <div className="dedupe-head">
                  <span className="dedupe-tag">保留</span>
                  <span className="truncate" title={g.keep.path}>{g.keep.file_name}</span>
                  <span className="muted">{formatBytes(g.size_bytes)}</span>
                </div>
                {g.others.map((o) => (
                  <label className="dedupe-row" key={o.id}>
                    <input type="checkbox" checked={picked.has(o.id)} onChange={() => toggle(o.id)} />
                    <span className="truncate" title={o.path}>{o.path}</span>
                    <span className="muted">
                      {o.exists ? `看过 ${o.watch_count} 次` : "文件已不在盘上"}
                    </span>
                  </label>
                ))}
              </div>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
