import { useRef, useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import type { Collection, Tag } from "../types";

export type ViewKey =
  | "all" | "gallery" | "recent" | "favorites" | "watchlist" | "unwatched" | "completed" | "trash";

const VIEWS: { key: ViewKey; label: string; icon: Parameters<typeof Icon>[0]["name"] }[] = [
  { key: "all", label: "全部视频", icon: "grid" },
  { key: "gallery", label: "合集画廊", icon: "sparkle" },
  { key: "recent", label: "最近观看", icon: "clock" },
  { key: "favorites", label: "收藏", icon: "heart" },
  { key: "watchlist", label: "待看清单", icon: "list" },
  { key: "unwatched", label: "未看", icon: "eye" },
  { key: "completed", label: "已看完", icon: "check" },
];

interface Props {
  view: ViewKey;
  onView: (v: ViewKey) => void;
  tags: Tag[];
  activeTag: string | null;
  onTag: (t: string | null) => void;
  collections: Collection[];
  activeCollection: string | null;
  onCollection: (c: string | null) => void;
  onCreateCollection: () => void;
  onManageCollections: () => void;
  onReordered?: () => void;
  onStats: () => void;
  onTags: () => void;
  onDownload: () => void;
  onEdit: () => void;
  onSettings: () => void;
  statsOpen: boolean;
  tagsOpen: boolean;
  downloadOpen: boolean;
  editOpen: boolean;
}

export function Sidebar(props: Props) {
  const [tagGroupsOpen, setTagGroupsOpen] = useState<Record<string, boolean>>({});
  const dragCollRef = useRef<number | null>(null);
  const groups = new Map<string, Tag[]>();
  for (const tag of props.tags) {
    const key = tag.category || "未分类";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(tag);
  }
  const sorted = [...groups.entries()].sort(([a], [b]) => {
    if (a === "未分类") return -1;
    if (b === "未分类") return 1;
    return a.localeCompare(b, "zh-CN");
  });

  // 目录按分组归拢：有分组的按名字排，未分组的放最后
  const byGroup = new Map<string, Collection[]>();
  for (const c of props.collections.filter((x) => x.kind === "custom")) {
    const key = c.group_name || "";
    if (!byGroup.has(key)) byGroup.set(key, []);
    byGroup.get(key)!.push(c);
  }
  const collectionGroups = [...byGroup.entries()].sort(([a], [b]) => {
    if (!a) return 1;
    if (!b) return -1;
    return a.localeCompare(b, "zh-CN");
  });

  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-mark"><Icon name="vault" size={22} /></span>
        <span className="brand-name">影匣</span>
        <span className="brand-sub">LOCAL VIDEO VAULT</span>
      </div>

      <nav className="nav-group">
        <div className="nav-caption">资料库</div>
        {VIEWS.map((v) => (
          <button
            key={v.key}
            className={`nav-item ${props.view === v.key && !props.activeTag && !props.activeCollection && !props.statsOpen && !props.tagsOpen ? "active" : ""}`}
            onClick={() => props.onView(v.key)}
          >
            <Icon name={v.icon} size={17} />
            <span>{v.label}</span>
          </button>
        ))}
        <button
          className={`nav-item ${props.view === "trash" ? "active" : ""}`}
          onClick={() => props.onView("trash")}
        >
          <Icon name="trash" size={17} />
          <span>回收站</span>
        </button>
      </nav>

      <nav className="nav-group nav-scroll">
        <div className="nav-caption nav-caption-row">
          <span>标签</span>
          <button className="caption-action" onClick={props.onTags} title="管理标签">
            <Icon name="settings" size={14} />
          </button>
        </div>
        {sorted.map(([category, tags]) => {
          const open = tagGroupsOpen[category] ?? true;
          return (
            <div key={category} className="tag-group">
              <button
                className="tag-group-head"
                onClick={() => setTagGroupsOpen((s) => ({ ...s, [category]: !open }))}
              >
                <Icon name={open ? "down" : "right"} size={13} />
                <span>{category}</span>
                <span className="tag-group-count">{tags.length}</span>
              </button>
              {open && tags.slice(0, 40).map((tag) => (
                <button
                  key={tag.id}
                  className={`nav-item nav-item-sm ${props.activeTag === tag.name ? "active" : ""}`}
                  onClick={() => props.onTag(tag.name)}
                  title={`${tag.name} · ${tag.count} 条`}
                >
                  <span className="tag-dot" style={{ opacity: 0.35 + Math.min(tag.weight, 5) * 0.13 }} />
                  <span className="truncate">{tag.name}</span>
                  <span className="nav-count">{tag.count}</span>
                </button>
              ))}
            </div>
          );
        })}
        {props.tags.length === 0 && <div className="nav-empty">扫描后自动生成标签树</div>}
      </nav>

      {props.collections.length > 0 && (
      <nav className="nav-group nav-short">
        <div className="nav-caption nav-caption-row">
          <span>收藏夹</span>
          <span className="caption-actions">
            <button className="caption-action" onClick={props.onCreateCollection} title="新建收藏夹">
              <Icon name="plus" size={14} />
            </button>
            <button className="caption-action" onClick={props.onManageCollections} title="管理收藏夹">
              <Icon name="settings" size={14} />
            </button>
          </span>
        </div>
        {collectionGroups.map(([group, list]) => (
          <div key={group || "__none"} className="tag-group">
            {group && (
              <div className="tag-group-head static">
                <Icon name="folder" size={12} />
                <span>{group}</span>
                <span className="tag-group-count">{list.length}</span>
              </div>
            )}
            {list.map((c) => (
              <button
                key={c.id}
                className={`nav-item nav-item-sm ${props.activeCollection === c.name ? "active" : ""}`}
                onClick={() => props.onCollection(c.name)}
                title={c.group_name ? `${c.group_name} / ${c.name} · ${c.count} 条（拖拽可排序）` : `${c.name} · ${c.count} 条（拖拽可排序）`}
                draggable
                onDragStart={(e) => {
                  dragCollRef.current = c.id;
                  e.dataTransfer.effectAllowed = "move";
                  e.currentTarget.classList.add("dragging");
                }}
                onDragOver={(e) => {
                  if (dragCollRef.current == null || dragCollRef.current === c.id) return;
                  e.preventDefault();
                  e.currentTarget.classList.add("drag-over");
                }}
                onDragLeave={(e) => e.currentTarget.classList.remove("drag-over")}
                onDrop={(e) => {
                  e.currentTarget.classList.remove("drag-over");
                  const dragId = dragCollRef.current;
                  dragCollRef.current = null;
                  if (dragId == null || dragId === c.id) return;
                  e.preventDefault();
                  const ids = props.collections.filter((x) => x.kind === "custom").map((x) => x.id);
                  const from = ids.indexOf(dragId);
                  const to = ids.indexOf(c.id);
                  if (from < 0 || to < 0) return;
                  ids.splice(to, 0, ids.splice(from, 1)[0]);
                  void api.reorderCollections(ids).then(() => props.onReordered?.());
                }}
                onDragEnd={(e) => {
                  e.currentTarget.classList.remove("dragging");
                  e.currentTarget.classList.remove("drag-over");
                }}
              >
                <Icon name="folder" size={15} />
                <span className="truncate">{c.name}</span>
                <span className="nav-count">{c.count}</span>
              </button>
            ))}
          </div>
        ))}
      </nav>
      )}

      <div className="sidebar-footer">
        <button className={`nav-item ${props.downloadOpen ? "active" : ""}`} onClick={props.onDownload}>
          <Icon name="down" size={17} /><span>下载器</span>
        </button>
        <button className={`nav-item ${props.editOpen ? "active" : ""}`} onClick={props.onEdit}>
          <Icon name="cut" size={17} /><span>剪辑</span>
        </button>
        <button className={`nav-item ${props.statsOpen ? "active" : ""}`} onClick={props.onStats}>
          <Icon name="chart" size={17} /><span>统计面板</span>
        </button>
        <button className={`nav-item ${props.tagsOpen ? "active" : ""}`} onClick={props.onTags}>
          <Icon name="tag" size={17} /><span>标签中心</span>
        </button>
        <button className={`nav-item ${false ? "active" : ""}`} onClick={props.onSettings}>
          <Icon name="settings" size={17} /><span>设置</span>
        </button>
      </div>
    </aside>
  );
}
