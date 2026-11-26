import { useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import type { Collection } from "../types";

/** 目录管理：新建 / 改名 / 分组 / 软删除 / 恢复 / 彻底删除 */
export function CollectionManager({
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
