import type { DragEvent, MouseEvent } from "react";
import { useEffect, useRef, useState } from "react";
import { Icon } from "./Icons";
import { useCoverSrc } from "../cover";
import { formatBytes, formatDuration, relativeTime } from "../format";
import type { VideoItem } from "../types";

interface Props {
  video: VideoItem;
  selected: boolean;
  onOpen: (v: VideoItem, edit: boolean) => void;
  onPlay: (v: VideoItem) => void;
  onSelect: (v: VideoItem, ev: MouseEvent) => void;
  /** 画廊双击行为（设置页可改）：默认直接播放，detail = 打开详情 */
  doubleClick?: "play" | "detail";
  /** 卡片下方是否显示文件名与元信息（设置页可关，纯封面墙） */
  showInfo?: boolean;
  /** 收藏夹视图手动排序时的拖拽能力（缺省 = 不可拖） */
  dragEnabled?: boolean;
  onDragStart?: (v: VideoItem, e: DragEvent<HTMLElement>) => void;
  onDragOver?: (v: VideoItem, e: DragEvent<HTMLElement>) => void;
  onDragLeave?: (e: DragEvent<HTMLElement>) => void;
  onDrop?: (v: VideoItem, e: DragEvent<HTMLElement>) => void;
  onDragEnd?: (e: DragEvent<HTMLElement>) => void;
}

export function VideoCard({
  video, selected, onOpen, onPlay, onSelect,
  doubleClick = "play", showInfo = true,
  dragEnabled, onDragStart, onDragOver, onDragLeave, onDrop, onDragEnd,
}: Props) {
  const progress = video.duration_sec > 0
    ? Math.min(100, (video.progress_sec / video.duration_sec) * 100)
    : 0;
  // 卡片进视口附近（上下 600px）才请求中帧封面；大文件夹首屏不再几十个 ffmpeg 排队
  const cardRef = useRef<HTMLElement | null>(null);
  const [near, setNear] = useState(false);
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    if (typeof IntersectionObserver === "undefined") { setNear(true); return; }
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        setNear(true);
        io.disconnect();
      }
    }, { rootMargin: "600px 0px" });
    io.observe(el);
    return () => io.disconnect();
  }, []);
  // 自选封面 > 接触表 > 中间帧兜底（后端懒生成），失败回占位图
  const cover = useCoverSrc(video, near);
  const isArchive = video.entry_index > 0;

  // 单击开详情、双击直接播放：单击先挂一个 220ms 的定时器，双击时撤掉，避免详情先跳出来闪一下
  const openTimer = useRef<number | null>(null);
  const openDeferred = () => {
    if (openTimer.current) window.clearTimeout(openTimer.current);
    openTimer.current = window.setTimeout(() => {
      openTimer.current = null;
      onOpen(video, false);
    }, 220);
  };
  const playNow = () => {
    if (openTimer.current) {
      window.clearTimeout(openTimer.current);
      openTimer.current = null;
    }
    if (doubleClick === "play") onPlay(video);
    else onOpen(video, false);
  };

  return (
    <article
      ref={cardRef}
      className={`video-card ${selected ? "selected" : ""}`}
      data-video-id={video.id}
      onClick={(e) => onSelect(video, e)}
      onDoubleClick={playNow}
      draggable={dragEnabled || undefined}
      onDragStart={(e) => onDragStart?.(video, e)}
      onDragOver={(e) => onDragOver?.(video, e)}
      onDragLeave={(e) => onDragLeave?.(e)}
      onDrop={(e) => onDrop?.(video, e)}
      onDragEnd={(e) => onDragEnd?.(e)}
    >
      <div className="card-thumb" onClick={openDeferred}>
        {cover ? (
          <img src={cover} alt="" loading="lazy" />
        ) : (
          <div className="thumb-placeholder">
            <Icon name="play" size={30} />
            <span className="thumb-format">{video.container.toUpperCase()}</span>
          </div>
        )}
        <button
          className="card-play"
          title="播放"
          onClick={(e) => { e.stopPropagation(); onPlay(video); }}
        >
          <Icon name="play" size={18} />
        </button>
        {video.favorite && <span className="card-badge fav"><Icon name="heart" size={13} /></span>}
        {isArchive && <span className="card-badge archive" title={`压缩包内第 ${video.entry_index + 1} / ${video.entry_count} 个`}>#{video.entry_index + 1}</span>}
        {video.rating > 0 && <span className="card-badge rating"><Icon name="star" size={12} />{video.rating}</span>}
        {progress > 0 && progress < 96 && (
          <div className="card-progress"><span style={{ width: `${progress}%` }} /></div>
        )}
        {video.completed && <span className="card-done"><Icon name="check" size={13} /></span>}
      </div>
      {showInfo && <div className="card-body" onClick={openDeferred}>
        <div className="card-title" title={video.file_name}>{video.file_name}</div>
        <div className="card-meta">
          <span>{formatDuration(video.duration_sec)}</span>
          <span className="dot-sep">·</span>
          <span>{formatBytes(video.size_bytes)}</span>
          {video.watch_count > 0 && (
            <>
              <span className="dot-sep">·</span>
              <span title={`看过 ${video.watch_count} 次`}><Icon name="eye" size={12} /> {video.watch_count}</span>
            </>
          )}
        </div>
        {/* 显示文件本身的修改时间：以前这里显示入库时间，全库都是「一天前」没有意义 */}
        <div className="card-sub" title={video.mtime > 0 ? "文件修改时间" : "加入影匣的时间"}>
          {video.mtime > 0 ? `改于 ${relativeTime(video.mtime)}` : `加入于 ${relativeTime(video.added_at)}`}
        </div>
      </div>}
    </article>
  );
}
