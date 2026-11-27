/**
 * 选中逻辑的小工具：纯函数，方便在别处复用与推理。
 * 对齐 Windows 资源管理器的习惯：普通单击=单选、Ctrl=切换、Shift=区间、空白拖拽=框选。
 */

/** 两点归一化成矩形（client 坐标）。 */
export function rectFromPoints(x0: number, y0: number, x1: number, y1: number) {
  return {
    left: Math.min(x0, x1),
    right: Math.max(x0, x1),
    top: Math.min(y0, y1),
    bottom: Math.max(y0, y1),
  };
}

/** 两个矩形是否相交。 */
export function rectsIntersect(
  a: { left: number; top: number; right: number; bottom: number },
  b: { left: number; top: number; right: number; bottom: number },
): boolean {
  return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
}

/**
 * 区间选择：按 `order` 里的顺序取 anchor → target（含两端）。
 * 任一端不在当前列表里（比如翻页后锚点已经滚出去了）就退化成只选 target，
 * 不会因为找不到锚点就整段乱选。
 */
export function rangeSelection(
  order: readonly number[],
  anchor: number | null,
  target: number,
): number[] {
  const ti = order.indexOf(target);
  if (ti < 0) return [target];
  const ai = anchor === null ? -1 : order.indexOf(anchor);
  if (ai < 0) return [target];
  const [from, to] = ai <= ti ? [ai, ti] : [ti, ai];
  return order.slice(from, to + 1);
}
