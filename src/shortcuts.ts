/**
 * 快捷键系统（默认**不绑定**任何键，全部留给用户在设置里自己配）。
 *
 * 设计：
 * - 动作表（ACTIONS）是唯一事实源：界面按 group 分组渲染，处理器在 App 里按 id 分派；
 * - 绑定存 localStorage（`vv-keymap`），形如 { "sort.folder": "Ctrl+1" }；
 * - 组合键规范化后比较（Ctrl/Alt/Shift/Meta + 主键），录制时按物理按下的键算；
 * - 输入框聚焦、弹窗打开时统一不触发（避免和打字打架）。
 */

export type ActionId =
  | "sort.added"
  | "sort.folder"
  | "sort.title"
  | "sort.size"
  | "sort.toggleDir"
  | "play.toggle"
  | "play.speedUp"
  | "play.speedDown"
  | "select.all"
  | "select.clear"
  | "fav.toggle"
  | "folder.up"
  | "immersive.toggle"
  | "edit.rename"
  | "edit.move"
  | "edit.trash"
  | "boss.hide"
  | "boss.recall";

export interface ActionDef {
  id: ActionId;
  label: string;
  group: "排序" | "播放" | "选择与收藏" | "文件" | "界面";
  hint?: string;
}

export const ACTIONS: ActionDef[] = [
  { id: "sort.added", label: "排序：最近加入", group: "排序" },
  { id: "sort.folder", label: "排序：文件夹", group: "排序", hint: "切到文件夹模式（只显示文件夹网格）" },
  { id: "sort.title", label: "排序：文件名", group: "排序" },
  { id: "sort.size", label: "排序：大小", group: "排序" },
  { id: "sort.toggleDir", label: "排序方向正/反", group: "排序" },
  { id: "play.toggle", label: "播放 / 暂停", group: "播放" },
  { id: "play.speedUp", label: "倍速 +0.25", group: "播放" },
  { id: "play.speedDown", label: "倍速 −0.25", group: "播放" },
  { id: "select.all", label: "全选当前页", group: "选择与收藏" },
  { id: "select.clear", label: "清空选择", group: "选择与收藏" },
  { id: "fav.toggle", label: "收藏 / 取消收藏", group: "选择与收藏", hint: "作用于选中项；没勾选时作用于详情里的那条" },
  { id: "folder.up", label: "返回上一级文件夹", group: "文件" },
  { id: "edit.rename", label: "重命名", group: "文件", hint: "选中 1 条时可用" },
  { id: "edit.move", label: "移动到…", group: "文件", hint: "选中若干条时可用" },
  { id: "edit.trash", label: "移入回收站", group: "文件" },
  { id: "immersive.toggle", label: "沉浸播放开关", group: "界面" },
  { id: "boss.hide", label: "老板键：藏窗", group: "界面", hint: "把窗口整个藏起来，像没开这个软件" },
  { id: "boss.recall", label: "老板键：全局呼回", group: "界面", hint: "系统级热键（窗口藏起后只有它能唤回）" },
];

export type KeyMap = Partial<Record<ActionId, string>>;

const STORE = "vv-keymap";

export function loadKeyMap(): KeyMap {
  try {
    const raw = localStorage.getItem(STORE);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as KeyMap;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

export function saveKeyMap(map: KeyMap) {
  try {
    localStorage.setItem(STORE, JSON.stringify(map));
  } catch {
    /* 存不下只影响本次会话，不抛 */
  }
}

const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta", "CapsLock", "Dead"]);

/** 把一次按键事件规范成 "Ctrl+Shift+F" 这种字符串；按住修饰键本身的返回 null。 */
export function comboFromEvent(e: KeyboardEvent | React.KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Meta");
  const main = normalizeKey(e.key);
  if (!main) return null;
  return [...mods, main].join("+");
}

function normalizeKey(key: string): string {
  if (!key) return "";
  if (key === " ") return "Space";
  if (key === "Escape") return "Esc";
  if (key === "ArrowUp") return "↑";
  if (key === "ArrowDown") return "↓";
  if (key === "ArrowLeft") return "←";
  if (key === "ArrowRight") return "→";
  if (key.length === 1) return key.toUpperCase();
  return key;
}

/** 反向查表：这个组合键绑给哪个动作。 */
export function actionForCombo(map: KeyMap, combo: string): ActionId | null {
  for (const [id, c] of Object.entries(map)) {
    if (c === combo) return id as ActionId;
  }
  return null;
}

/** 绑定一个组合键：先把该组合上原有的绑定清掉，保证一键一动作。 */
export function bindAction(map: KeyMap, id: ActionId, combo: string): KeyMap {
  const next: KeyMap = { ...map };
  for (const key of Object.keys(next) as ActionId[]) {
    if (next[key] === combo) delete next[key];
  }
  next[id] = combo;
  return next;
}

export function unbindAction(map: KeyMap, id: ActionId): KeyMap {
  const next: KeyMap = { ...map };
  delete next[id];
  return next;
}
