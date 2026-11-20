import type { ReactElement, SVGProps } from "react";

type Name =
  | "vault" | "grid" | "play" | "pause" | "search" | "star" | "heart"
  | "clock" | "eye" | "check" | "trash" | "tag" | "folder" | "chart"
  | "settings" | "plus" | "x" | "down" | "left" | "right" | "refresh"
  | "volume" | "fullscreen" | "back" | "forward" | "camera" | "link"
  | "list" | "info" | "menu" | "filter" | "sparkle" | "external"
  | "cut" | "merge";

const PATHS: Record<Name, ReactElement> = {
  vault: <><rect x="3" y="4" width="18" height="16" rx="3"/><circle cx="12" cy="12" r="3.5"/><path d="M12 8.5V6M15.5 12H18"/></>,
  grid: <><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/></>,
  play: <path d="M8 5.5v13l10-6.5z" fill="currentColor" stroke="none"/>,
  pause: <><rect x="7" y="5" width="3.5" height="14" rx="1" fill="currentColor" stroke="none"/><rect x="13.5" y="5" width="3.5" height="14" rx="1" fill="currentColor" stroke="none"/></>,
  search: <><circle cx="10.5" cy="10.5" r="6.5"/><path d="m15.5 15.5 5 5"/></>,
  star: <path d="m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1-5.4-2.9-5.4 2.9 1-6.1L3.2 9.5l6.1-.9z"/>,
  heart: <path d="M12 20s-7-4.6-7-9.2A4.1 4.1 0 0 1 12 8a4.1 4.1 0 0 1 7 2.8C19 15.4 12 20 12 20z"/>,
  clock: <><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.5 2"/></>,
  eye: <><path d="M2.5 12S6 6.5 12 6.5 21.5 12 21.5 12 18 17.5 12 17.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.5"/></>,
  check: <path d="m5 12.5 4.5 4.5L19 7"/>,
  trash: <><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/><path d="M10 11v5M14 11v5"/></>,
  tag: <><path d="M3 12V5a2 2 0 0 1 2-2h7l9 9-9 9z"/><circle cx="7.5" cy="7.5" r="1"/></>,
  folder: <path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2h8.5A1.5 1.5 0 0 1 21 8.5v9A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5z"/>,
  chart: <><path d="M4 20V10M10 20V4M16 20v-7M22 20H2"/></>,
  settings: <><circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9 7 7M17 17l2.1 2.1M19.1 4.9 17 7M7 17l-2.1 2.1"/></>,
  plus: <path d="M12 5v14M5 12h14"/>,
  x: <path d="M6 6l12 12M18 6 6 18"/>,
  down: <path d="m6 9 6 6 6-6"/>,
  left: <path d="m15 5-7 7 7 7"/>,
  right: <path d="m9 5 7 7-7 7"/>,
  refresh: <><path d="M20 11a8 8 0 1 0-2.3 6.2"/><path d="M20 5v6h-6"/></>,
  volume: <><path d="M5 9v6h4l5 4V5L9 9z"/><path d="M17 9a4 4 0 0 1 0 6"/></>,
  fullscreen: <><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/></>,
  back: <><path d="M6 5v14M19 5 9 12l10 7z"/></>,
  forward: <><path d="M18 5v14M5 5l10 7-10 7z"/></>,
  camera: <><rect x="3" y="7" width="18" height="13" rx="2"/><circle cx="12" cy="13.5" r="3.5"/><path d="M8 7l1.5-2h5L16 7"/></>,
  link: <><path d="M10 13a5 5 0 0 0 7.5.5l2-2A5 5 0 0 0 12.5 4l-1 1"/><path d="M14 11a5 5 0 0 0-7.5-.5l-2 2A5 5 0 0 0 11.5 20l1-1"/></>,
  list: <><path d="M8 6h13M8 12h13M8 18h13"/><circle cx="4" cy="6" r="1"/><circle cx="4" cy="12" r="1"/><circle cx="4" cy="18" r="1"/></>,
  info: <><circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/></>,
  menu: <path d="M4 7h16M4 12h16M4 17h16"/>,
  filter: <path d="M3 5h18l-7 8v6l-4 2v-8z"/>,
  sparkle: <><path d="m12 3 1.6 5.4L19 10l-5.4 1.6L12 17l-1.6-5.4L5 10l5.4-1.6z"/><path d="m19 16 .8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8z"/></>,
  external: <><path d="M14 4h6v6M20 4l-9 9"/><path d="M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/></>,
  cut: <><circle cx="6" cy="6" r="2.6"/><circle cx="6" cy="18" r="2.6"/><path d="M8.3 7.7 20 19.5M8.3 16.3 20 4.5"/></>,
  merge: <><path d="M5 5v5a4 4 0 0 0 4 4h10"/><path d="M15 10l4 4-4 4"/><path d="M5 19v-3"/></>,
};

export function Icon({
  name,
  size = 18,
  ...rest
}: { name: Name; size?: number } & SVGProps<SVGSVGElement>) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      {PATHS[name]}
    </svg>
  );
}
