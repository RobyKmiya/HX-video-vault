import { Icon } from "./Icons";
import { formatBytes } from "../format";
import type { ImportStatus } from "../types";

/** 角落传输浮窗：队列位置 + 当前文件百分比 + 失败原因；完成后停几秒自退（有失败则留着） */
export function ImportHud({
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
