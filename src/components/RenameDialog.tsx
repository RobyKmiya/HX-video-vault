import { useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import type { VideoItem } from "../types";

/** 全局重命名弹窗：右键菜单 / 快捷键都走这里，详情抽屉里那份是内联版。 */
export function RenameDialog({
  video,
  onClose,
  onDone,
  notify,
}: {
  video: VideoItem;
  onClose: () => void;
  onDone: (v: VideoItem) => void;
  notify: (m: string) => void;
}) {
  const dot = video.file_name.lastIndexOf(".");
  const ext = dot > 0 ? video.file_name.slice(dot) : "";
  const [name, setName] = useState(dot > 0 ? video.file_name.slice(0, dot) : video.file_name);
  const [busy, setBusy] = useState(false);

  const go = async () => {
    const raw = name.trim();
    if (!raw) {
      notify("文件名不能为空");
      return;
    }
    setBusy(true);
    try {
      const v = await api.renameVideo(video.id, raw + ext);
      notify(`已改名为「${v.file_name}」`);
      onDone(v);
      onClose();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal rename-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="panel-head">
          <div>
            <h1>重命名</h1>
            <p>扩展名固定，只改主名{ext ? `（${ext}）` : ""}</p>
          </div>
          <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
        </div>
        <div className="modal-body">
          <div className="rename-row">
            <input
              className="rename-input"
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void go();
                if (e.key === "Escape") onClose();
              }}
              spellCheck={false}
            />
            <span className="rename-ext">{ext}</span>
          </div>
        </div>
        <div className="modal-actions">
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn primary" onClick={() => void go()} disabled={busy}>{busy ? "改名中…" : "确定"}</button>
        </div>
      </div>
    </div>
  );
}
