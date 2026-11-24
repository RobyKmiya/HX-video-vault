/**
 * 鼠标手势（画匣 gestures.ts 的视频版）。
 *
 * 画匣是漫画阅读器，分区点击天然对应「上一页/下一页」；
 * 视频播放器没有「页」的概念，所以这边**不做点击分区** ——
 * 单击画面保持「立即播放/暂停」，加了 220ms 延迟手感会发木。
 *
 * 只留三个槽位，都是视频里真的有意义的：
 *   双击画面 / Ctrl+滚轮上 / Ctrl+滚轮下
 *
 * 动作集复用播放器内部已有的操作，不另造一套实现。
 */

export type GestureSlot = "doubleClick" | "ctrlWheelUp" | "ctrlWheelDown";

export type GestureAction =
  | "toggleImmersive"
  | "toggleFullscreen"
  | "togglePlay"
  | "volumeUp"
  | "volumeDown"
  | "seekForward"
  | "seekBackward"
  | "none";

export type GestureMap = Record<GestureSlot, GestureAction>;

export const GESTURE_LABELS: Record<GestureSlot, string> = {
  doubleClick: "双击画面",
  ctrlWheelUp: "Ctrl + 滚轮上滚",
  ctrlWheelDown: "Ctrl + 滚轮下滚",
};

export const GESTURE_ACTION_LABELS: Record<GestureAction, string> = {
  toggleImmersive: "切换无边框沉浸",
  toggleFullscreen: "切换全屏",
  togglePlay: "播放 / 暂停",
  volumeUp: "音量 +10%",
  volumeDown: "音量 −10%",
  seekForward: "快进 10 秒",
  seekBackward: "快退 10 秒",
  none: "不响应",
};

export const DEFAULT_GESTURES: GestureMap = {
  doubleClick: "toggleImmersive",
  ctrlWheelUp: "volumeUp",
  ctrlWheelDown: "volumeDown",
};

const STORAGE_KEY = "vv.gestures";

export function loadGestures(): GestureMap {
  const merged: GestureMap = { ...DEFAULT_GESTURES };
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return merged;
    const parsed = JSON.parse(raw) as Partial<GestureMap>;
    for (const slot of Object.keys(DEFAULT_GESTURES) as GestureSlot[]) {
      if (typeof parsed[slot] === "string") {
        const action = parsed[slot] as GestureAction;
        if (action in GESTURE_ACTION_LABELS) merged[slot] = action;
      }
    }
  } catch {
    // 存坏了就用默认值
  }
  return merged;
}

export function saveGestures(map: GestureMap) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    // 存不了就只在本次会话生效
  }
}
