import { useEffect, useMemo, useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import type { MoveTarget } from "../types";

/**
 * 移动到库内文件夹。目标 = 库根 + 所有 auto 镜像夹（后端一次查回，点击即走）。
 * 同盘移动是元数据操作（毫秒级）；封面按视频 id 缓存，移动后不重抽，所以列表不闪。
 */
export function MoveDialog({
  ids,
  currentPath,
  onClose,
  onDone,
  notify,
}: {
  ids: number[];
  /** 任意一条视频的当前完整路径，用来标出「已在此」并顺手显示所在目录 */
  currentPath?: string;
  onClose: () => void;
  onDone: (moved: number) => void;
  notify: (m: string) => void;
}) {
  const [targets, setTargets] = useState<MoveTarget[]>([]);
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    api.moveTargets().then(setTargets).catch((e) => notify(String(e)));
  }, [notify]);

  const curDir = useMemo(() => {
    if (!currentPath) return "";
    const i = Math.max(currentPath.lastIndexOf("\\"), currentPath.lastIndexOf("/"));
    return i > 0 ? currentPath.slice(0, i) : currentPath;
  }, [currentPath]);

  const list = useMemo(() => {
    const k = q.trim().toLowerCase();
    if (!k) return targets;
    return targets.filter(
      (t) => t.name.toLowerCase().includes(k) || t.path.toLowerCase().includes(k),
    );
  }, [targets, q]);

  const go = async (t: MoveTarget) => {
    if (busy) return;
    if (curDir && t.path.toLowerCase() === curDir.toLowerCase()) {
      notify("已经在这个文件夹里了");
      return;
    }
    setBusy(t.path);
    try {
      const n = await api.moveVideos(ids, t.path);
      notify(`已移动 ${n} 个视频到「${t.name}」`);
      onDone(n);
      onClose();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal move-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="panel-head">
          <div>
            <h1>移动到…</h1>
            <p>{ids.length} 个视频 · 只在库目录内移动，同盘是瞬时的改名操作</p>
          </div>
          <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
        </div>
        <div className="modal-body">
          <input
            className="move-search"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="搜索文件夹…"
            autoFocus
          />
          <div className="move-list">
            {list.map((t) => {
              const here = !!curDir && t.path.toLowerCase() === curDir.toLowerCase();
              return (
                <button
                  key={t.path}
                  className={`move-row ${here ? "here" : ""} ${busy === t.path ? "busy" : ""}`}
                  onClick={() => void go(t)}
                  disabled={!!busy}
                  title={t.path}
                >
                  <Icon name="folder" size={15} />
                  <span className="truncate">{t.name}</span>
                  {here && <span className="group-chip">当前</span>}
                  {t.count > 0 && <span className="nav-count">{t.count}</span>}
                  {busy === t.path && <span className="muted small">移动中…</span>}
                </button>
              );
            })}
            {list.length === 0 && <div className="nav-empty">没有匹配的文件夹</div>}
          </div>
          <p className="hint">目标里已有同名文件会自动加「 (2)」，不覆盖；移动后封面照旧（缓存跟着视频走）。</p>
        </div>
      </div>
    </div>
  );
}
