import { useState } from "react";
import { Icon } from "./Icons";
import type { SheetStatus } from "../types";

/** 预览图任务面板：全量生成 / 暂停 / 续跑 / 清空重建 */
export function SheetPanel({
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
