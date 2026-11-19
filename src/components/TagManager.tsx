import { useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import type { Tag } from "../types";

interface Props {
  tags: Tag[];
  onClose: () => void;
  onChanged: () => void;
  onError: (msg: string) => void;
}

export function TagManager({ tags, onClose, onChanged, onError }: Props) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [name, setName] = useState("");
  const [category, setCategory] = useState("");
  const [weight, setWeight] = useState(0);
  const [draft, setDraft] = useState<Record<number, number>>({});

  const groups = new Map<string, Tag[]>();
  for (const tag of tags) {
    const key = tag.category || "未分类";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(tag);
  }
  const sorted = [...groups.entries()].sort(([a], [b]) => {
    if (a === "未分类") return -1;
    if (b === "未分类") return 1;
    return a.localeCompare(b, "zh-CN");
  });

  const add = async () => {
    if (!name.trim()) return;
    try {
      await api.createTag(name.trim(), category.trim(), weight);
      setName(""); setWeight(0); onChanged();
    } catch (e) { onError(String(e)); }
  };

  const saveWeight = async (tag: Tag) => {
    const w = draft[tag.id] ?? tag.weight;
    if (w === tag.weight) return;
    try { await api.updateTag(tag.id, tag.name, tag.category, w); onChanged(); }
    catch (e) { onError(String(e)); }
  };

  return (
    <section className="stats-panel">
      <div className="panel-head">
        <div><h1>标签中心</h1><p>未分类永远排最前，改完不用再滑到底</p></div>
        <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
      </div>

      <div className="panel-card tag-create">
        <div className="field"><label>新标签名</label><input value={name} onChange={(e) => setName(e.target.value)} placeholder="例如：已整理" /></div>
        <div className="field"><label>大类</label><input value={category} onChange={(e) => setCategory(e.target.value)} placeholder="例如：状态" /></div>
        <div className="field field-sm"><label>权重</label><input type="number" min={0} max={9} value={weight} onChange={(e) => setWeight(Number(e.target.value))} /></div>
        <button className="btn primary" onClick={add}><Icon name="plus" size={15} />新建</button>
      </div>

      {sorted.map(([categoryName, list]) => {
        const isOpen = open[categoryName] ?? true;
        return (
          <div className="panel-card tag-category" key={categoryName}>
            <button className="category-head" onClick={() => setOpen((s) => ({ ...s, [categoryName]: !isOpen }))}>
              <Icon name={isOpen ? "down" : "right"} size={15} />
              <strong>{categoryName}</strong>
              <span className="muted">{list.length} 个标签</span>
            </button>
            {isOpen && (
              <div className="tag-rows">
                {list.map((tag) => (
                  <div className="tag-row" key={tag.id}>
                    <span className="tag-row-name">{tag.name}</span>
                    <span className="tag-row-count">{tag.count} 条</span>
                    <label className="weight-control">
                      权重
                      <input
                        type="range" min={0} max={9} step={1}
                        value={draft[tag.id] ?? tag.weight}
                        onChange={(e) => setDraft((d) => ({ ...d, [tag.id]: Number(e.target.value) }))}
                        onMouseUp={() => void saveWeight(tag)}
                        onTouchEnd={() => void saveWeight(tag)}
                      />
                      <b>{draft[tag.id] ?? tag.weight}</b>
                    </label>
                  </div>
                ))}
              </div>
            )}
          </div>
        );
      })}
      {tags.length === 0 && <div className="empty"><Icon name="tag" size={32} /><p>还没有标签</p></div>}
    </section>
  );
}
