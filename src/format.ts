export function formatBytes(n: number): string {
  if (!n || n <= 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1);
  return `${(n / 1024 ** i).toFixed(i >= 3 ? 2 : 1)} ${units[i]}`;
}

export function formatDuration(sec: number): string {
  if (!sec || sec <= 0) return "—";
  const s = Math.floor(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
  return `${m}:${String(ss).padStart(2, "0")}`;
}

export function formatDate(ts: number): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function relativeTime(ts: number): string {
  if (!ts) return "从未";
  const delta = Date.now() / 1000 - ts;
  if (delta < 60) return "刚刚";
  if (delta < 3600) return `${Math.floor(delta / 60)} 分钟前`;
  if (delta < 86400) return `${Math.floor(delta / 3600)} 小时前`;
  if (delta < 86400 * 30) return `${Math.floor(delta / 86400)} 天前`;
  return formatDate(ts);
}

/**
 * 缩略图容器的宽高比：跟随视频本身的画面比例，
 * 这样竖屏视频不会被 16:9 的框裁掉上下大半。
 * 元数据缺失时退回 16:9；极端比例做上下限收敛，避免卡片被拉得过长。
 */
export function thumbRatio(width: number, height: number): string {
  if (!width || !height || width <= 0 || height <= 0) return "16 / 9";
  const r = Math.min(2.4, Math.max(0.5, width / height));
  return `${r.toFixed(4)}`;
}
