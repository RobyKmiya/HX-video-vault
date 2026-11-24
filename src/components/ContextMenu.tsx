import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

/**
 * 通用右键菜单（画匣 ReaderMenu 同款协议）。
 *
 * 三个要点：
 * 1. 贴边回弹 —— 菜单快出屏时往回收，别让用户够不到。
 *    用 useLayoutEffect 而不是 useEffect：浏览器绘制前就修正，
 *    用户看不到菜单先出现在屏外再跳回来。
 * 2. 三向关闭 —— 点外面、按 Esc、别处右键。
 *    Esc 要 stopPropagation：播放器的 Esc 是「退出播放」，
 *    不拦住会变成「关菜单的同时把视频也退了」。
 * 3. 分组用 dividerBefore 表达，保持扁平数组，实现最简。
 */

export interface MenuEntry {
  label: string;
  /** 左侧小图标 */
  icon?: ReactNode;
  /** 右侧快捷键提示（纯展示） */
  hint?: string;
  /** 这一条之前画一条分隔线 */
  dividerBefore?: boolean;
  /** 分组标题（配合 dividerBefore 用；画在分隔线位置，不带边框） */
  groupTitle?: string;
  disabled?: boolean;
  /** 危险操作（删除一类） */
  danger?: boolean;
  /** 当前项打勾 */
  checked?: boolean;
  onClick: () => void;
}

interface Props {
  x: number;
  y: number;
  entries: MenuEntry[];
  /** 可选：菜单顶部的一行说明（比如影片名） */
  header?: string;
  onClose: () => void;
}

export function ContextMenu({ x, y, entries, header, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setPos({
      x: Math.min(Math.max(4, x), window.innerWidth - rect.width - 4),
      y: Math.min(Math.max(4, y), window.innerHeight - rect.height - 4),
    });
  }, [x, y]);

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [onClose]);

  return (
    <>
      <div
        className="menu-shadow"
        onMouseDown={onClose}
        onContextMenu={(e) => { e.preventDefault(); onClose(); }}
      />
      <div ref={ref} className="context-menu player-ctx" style={{ left: pos.x, top: pos.y }} role="menu">
        {header && <div className="ctx-head">{header}</div>}
        {entries.map((entry, index) => (
          <div key={`${entry.label}-${index}`}>
            {entry.dividerBefore && (
              entry.groupTitle
                ? <div className="ctx-caption ctx-caption-title">{entry.groupTitle}</div>
                : <div className="ctx-caption" />
            )}
            <button
              className={entry.danger ? "danger" : ""}
              disabled={entry.disabled}
              role="menuitem"
              onClick={() => { onClose(); entry.onClick(); }}
            >
              {entry.icon}
              <span className="ctx-label">{entry.label}</span>
              {entry.checked && <span className="ctx-check">✓</span>}
              {entry.hint && <span className="ctx-hint">{entry.hint}</span>}
            </button>
          </div>
        ))}
      </div>
    </>
  );
}
