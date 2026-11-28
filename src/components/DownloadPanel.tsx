import { useEffect, useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import { formatBytes, relativeTime } from "../format";
import type { DownloadTask, ResolveReport, Tag } from "../types";

interface Props {
  tags: Tag[];
  onClose: () => void;
  onImported: () => void;
  notify: (m: string) => void;
}

export function DownloadPanel({ tags, onClose, onImported, notify }: Props) {
  const [url, setUrl] = useState("");
  const [resolving, setResolving] = useState(false);
  const [report, setReport] = useState<ResolveReport | null>(null);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [tasks, setTasks] = useState<DownloadTask[]>([]);
  const [tagText, setTagText] = useState("");

  const refresh = () => api.downloads().then(setTasks).catch((e) => notify(String(e)));
  useEffect(() => {
    refresh();
    void api.startDownloads().catch(() => {});
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) => {
      return listen("download-progress", (e) => {
        const p = e.payload as {
          id?: number; state?: string; event?: string;
          ingested?: boolean; folderNo?: number; slotNo?: number; error?: string;
        };
        if (p.state === "done") {
          if (p.ingested && p.folderNo) {
            notify(`已入库：${p.folderNo} 号夹 第 ${p.slotNo} 条（标签 / 原链接 / 作者已挂上）`);
            onImported();
          } else if (p.ingested === false) {
            // 后端入库失败才退回扫库兜底
            notify(p.error ? `入库失败：${p.error}` : "下载完成，正在扫描入库…");
            void api.scan({ root: "", recursive: true }).then(onImported).catch(() => {});
          }
        }
        void refresh();
      });
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; }).catch(() => {});
    // 联网恢复后自动续跑：监听 online 事件 + 定时检查
    const onOnline = () => { void api.startDownloads().then(refresh).catch(() => {}); };
    window.addEventListener("online", onOnline);
    const timer = window.setInterval(() => {
      void api.downloads().then((list) => {
        if (list.some((t) => t.state === "paused" || t.state === "queued")) onOnline();
      }).catch(() => {});
    }, 30000);
    return () => { cancelled = true; unlisten?.(); window.removeEventListener("online", onOnline); window.clearInterval(timer); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const doResolve = async () => {
    if (!url.trim()) return;
    setResolving(true); setReport(null);
    try {
      const r = await api.resolveLink(url.trim());
      setReport(r);
      // 默认只勾最高画质那一个：一页多画质是同一部片，全勾会重复下载
      const best = r.candidates.findIndex((c) => c.kind === "video");
      setPicked(new Set(best >= 0 ? [best] : []));
      if (!r.note.startsWith("发现")) notify(r.note);
    } catch (e) { notify(String(e)); }
    finally { setResolving(false); }
  };

  const startDownload = async (candidates = report?.candidates ?? []) => {
    const chosen = candidates.filter((_, i) => picked.has(i));
    if (chosen.length === 0 && report) { notify("至少勾选一个媒体"); return; }
    const list = chosen.length ? chosen : [{ url: url.trim(), name: report?.title || "download", kind: "video" }];
    // 解析出的作品标签 + 手填标签一起打包，去重保序
    const extra = tagText.split(/[，,、;；\s]+/).map((s) => s.trim()).filter(Boolean);
    const packed = [...(report?.tags ?? []), ...extra].filter((t, i, arr) => arr.indexOf(t) === i).join(",");
    try {
      let queued = 0;
      for (const c of list) {
        const r = await api.enqueueDownload({
          url: c.url,
          page_url: url.trim(),
          title: report?.title || c.name,
          site: report?.site || "",
          author: report?.author || "",
          tags: packed,
          note: report?.note || "",
        });
        if (r.duplicate) { notify(r.message); continue; }
        queued += 1;
      }
      await api.startDownloads();
      if (queued > 0) notify(`已加入 ${queued} 个下载任务，完成后自动入库打标`);
      setReport(null); setUrl(""); await refresh();
    } catch (e) { notify(String(e)); }
  };

  const stateLabel: Record<string, string> = {
    queued: "排队中", running: "下载中", paused: "已暂停", done: "已完成", failed: "失败", canceled: "已取消",
  };

  return (
    <section className="stats-panel">
      <div className="panel-head">
        <div><h1>下载器</h1><p>贴入网页链接 → 解析媒体 → 下载 → 自动入库打标并收进编号夹；断网保留分片，联网后自动续跑</p></div>
        <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
      </div>

      <div className="panel-card">
        <div className="download-input">
          <input
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") void doResolve(); }}
            placeholder="https://example.com/post/123  或媒体直链 .mp4 / .m3u8"
          />
          <button className="btn primary" onClick={() => void doResolve()} disabled={resolving || !url.trim()}>
            <Icon name="link" size={15} />{resolving ? "解析中…" : "解析链接"}
          </button>
        </div>
        <div className="field" style={{ marginTop: 12 }}>
          <label>自动打上的标签（逗号分隔；解析出的作品标签会一并打包）</label>
          <input value={tagText} onChange={(e) => setTagText(e.target.value)} placeholder="例如：B站, 直播录像" />
        </div>
        {tags.length > 0 && (
          <div className="tag-pills" style={{ marginTop: 8 }}>
            {tags.slice(0, 12).map((t) => (
              <button key={t.id} className="pill" onClick={() => setTagText((s) => s ? `${s}, ${t.name}` : t.name)}>+ {t.name}</button>
            ))}
          </div>
        )}
        <p className="hint">解析只读取公开页面 HTML，不上传本机任何数据；遇到需要登录/脚本渲染的站点，请直接贴媒体直链。</p>
      </div>

      {report && (
        <div className="panel-card">
          <div className="resolve-head">
            <div>
              <h2>{report.title || report.site}</h2>
              <p className="muted">{report.note}</p>
            </div>
            <button className="btn primary" onClick={() => void startDownload()}>
              <Icon name="down" size={15} />下载勾选的 {picked.size} 项
            </button>
          </div>
          <div className="candidate-list">
            {report.candidates.map((c, i) => (
              <label key={`${c.url}-${i}`} className="candidate-row">
                <input type="checkbox" checked={picked.has(i)} onChange={() => setPicked((s) => {
                  const n = new Set(s); if (n.has(i)) n.delete(i); else n.add(i); return n;
                })} />
                <span className={`candidate-kind ${c.kind}`}>{c.kind === "video" ? "视频" : c.kind === "image" ? "图片" : "压缩包"}</span>
                <span className="truncate" title={c.url}>{c.name}</span>
              </label>
            ))}
            {report.candidates.length === 0 && (
              <button className="btn" onClick={() => void startDownload([])}>直接下载这个链接</button>
            )}
          </div>
        </div>
      )}

      <div className="panel-card">
        <div className="resolve-head"><h2>下载队列</h2>
          <div style={{ display: "flex", gap: 8 }}>
            <button className="btn small" onClick={async () => {
              try { await api.clearDoneDownloads(); await refresh(); } catch (e) { notify(String(e)); }
            }}>清空已完成</button>
            <button className="btn small" onClick={() => { void api.startDownloads().then(refresh); }}><Icon name="refresh" size={14} />续跑</button>
          </div>
        </div>
        {tasks.length === 0 ? <p className="muted">还没有下载任务</p> : (
          <div className="task-list">
            {tasks.map((t) => (
              <div className="task-row" key={t.id}>
                <span className={`task-state ${t.state}`}>{stateLabel[t.state] || t.state}</span>
                <div className="task-main">
                  <div className="task-title truncate" title={t.url}>{t.title || t.url}</div>
                  <div className="task-bar"><span style={{ width: `${Math.round(t.progress * 100)}%` }} /></div>
                  <div className="task-meta muted">
                    {t.site} · {formatBytes(t.bytes_done)}{t.bytes_total ? ` / ${formatBytes(t.bytes_total)}` : ""} · {relativeTime(t.created_at)}
                    {t.error ? ` · ${t.error}` : ""}
                  </div>
                </div>
                <button className="icon-btn" title="移除任务" onClick={async () => {
                  try { await api.removeDownload(t.id, false); await refresh(); } catch (e) { notify(String(e)); }
                }}><Icon name="x" size={15} /></button>
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
