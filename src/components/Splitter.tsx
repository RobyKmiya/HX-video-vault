import { useRef } from "react";
import type { PointerEvent as ReactPointerEvent, KeyboardEvent as ReactKeyboardEvent } from "react";

/**
 * 可拖动分界线（画匣 Splitter 同款）。
 *
 * 三个要点：
 * 1. setPointerCapture —— 指针移出元素外仍然继续拖；
 * 2. 拖动时给 body 加 is-splitting —— 全局禁用文本选中、光标统一成 col-resize；
 * 3. 键盘可达 —— ←→ 每次调 8px，回车收起/展开，Shift+回车复位。
 *    只靠鼠标拖的分割条对键盘用户等于不存在。
 *
 * direction：1 = 往右拖变大（左侧栏）；-1 = 往右拖变小（右侧抽屉）。
 */

interface Props {
  /** 当前尺寸（px），用于 aria 播报与键盘步进 */
  value: number;
  min: number;
  max: number;
  onChange: (next: number) => void;
  /** 双击 / Shift+回车：复位到默认宽度 */
  onReset: () => void;
  /** 双击 / 回车：收起或展开（不给就退回复位） */
  onToggle?: () => void;
  toggleTitle?: string;
  direction?: 1 | -1;
  label: string;
}

export function Splitter({
  value, min, max, onChange, onReset, onToggle, toggleTitle, direction = 1, label,
}: Props) {
  const dragRef = useRef<{ startX: number; startValue: number } | null>(null);

  const clamp = (n: number) => Math.min(max, Math.max(min, Math.round(n)));

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { startX: event.clientX, startValue: value };
    document.body.classList.add("is-splitting");
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const state = dragRef.current;
    if (!state) return;
    const next = clamp(state.startValue + (event.clientX - state.startX) * direction);
    if (next !== value) onChange(next);
  };

  const endDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return;
    dragRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    document.body.classList.remove("is-splitting");
  };

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      onChange(clamp(value - 8 * direction));
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      onChange(clamp(value + 8 * direction));
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (event.shiftKey) onReset();
      else (onToggle ?? onReset)();
    }
  };

  return (
    <div
      className="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-label={label}
      tabIndex={0}
      title={toggleTitle ?? "拖动调宽度，双击复位（←→ 微调，Shift+回车复位）"}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={(event) => {
        event.preventDefault();
        (onToggle ?? onReset)();
      }}
      onKeyDown={handleKeyDown}
    />
  );
}
