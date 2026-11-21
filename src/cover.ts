import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "./api";
import type { VideoItem } from "./types";

/**
 * 封面显示的统一入口，优先级：自选封面 > 接触表 > 中间帧兜底（后端懒生成）。
 * - 模块级 Promise 缓存：卡片与详情同一条视频只发一次请求；
 * - 前端限流 2 路并发，首屏几十张卡不会瞬间轰出几十个 ffmpeg；
 * - bustCover(id)：自选封面被替换/撤销后给 URL 加版本参数，防 WebView 图片缓存。
 */

const cache = new Map<number, Promise<string>>();
const busts: Record<number, number> = {};
let active = 0;
const waiting: (() => void)[] = [];

async function acquire() {
  while (active >= 2) await new Promise<void>((r) => waiting.push(r));
  active++;
}
function release() {
  active--;
  waiting.shift()?.();
}

/** 自选封面变化后调用（设置/撤销都算），让所有展示点拿到带新版本号的 URL。 */
export function bustCover(id: number) {
  busts[id] = Date.now();
}

function bustSuffix(id: number) {
  const v = busts[id];
  return v ? `?v=${v}` : "";
}

/** 有自选封面时直接给出 URL，否则返回空串（走中帧兜底 hook；接触表不再当封面）。 */
export function coverSrc(video: Pick<VideoItem, "id" | "cover" | "contact_sheet">): string {
  if (video.cover) return convertFileSrc(video.cover) + bustSuffix(video.id);
  return "";
}

/** 封面 URL hook：自选封面 > 中间帧（后端懒生成），兜底失败返回空串（调用方画占位）。 */
export function useCoverSrc(video: Pick<VideoItem, "id" | "cover" | "contact_sheet">): string {
  const own = coverSrc(video);
  const [fallback, setFallback] = useState("");
  useEffect(() => {
    if (own) {
      setFallback("");
      return;
    }
    let dead = false;
    let p = cache.get(video.id);
    if (!p) {
      p = (async () => {
        await acquire();
        try {
          return await api.videoCover(video.id);
        } finally {
          release();
        }
      })();
      cache.set(video.id, p);
    }
    p.then((path) => {
      if (!dead) setFallback(convertFileSrc(path) + bustSuffix(video.id));
    }).catch(() => {
      // 取不到（如时长读不出）就清缓存，下次渲染再试一次
      if (!dead) cache.delete(video.id);
    });
    return () => {
      dead = true;
    };
  }, [own, video.id, video.cover]);
  return own || fallback;
}
