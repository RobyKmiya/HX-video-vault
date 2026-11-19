import { convertFileSrc } from "@tauri-apps/api/core";
import type { MouseEvent } from "react";
import { Icon } from "./Icons";
import { formatBytes, formatDuration, relativeTime, thumbRatio } from "../format";
import type { VideoItem } from "../types";

interface Props {
  video: VideoItem;
  selected: boolean;
  onOpen: (v: VideoItem, edit: boolean) => void;
  onPlay: (v: VideoItem) => void;
  onSelect: (v: VideoItem, ev: MouseEvent) => void;
}

export function VideoCard({ video, selected, onOpen, onPlay, onSelect }: Props) {
  const progress = video.duration_sec > 0
    ? Math.min(100, (video.progress_sec / video.duration_sec) * 100)
    : 0;
  const cover = video.contact_sheet ? convertFileSrc(video.contact_sheet) : "";
  const isArchive = video.entry_index > 0;
  const ratio = thumbRatio(video.width, video.height);
  const portrait = video.width > 0 && video.height > video.width;

  return (
    <article
      className={`video-card ${selected ? "selected" : ""}`}
      onClick={(e) => onSelect(video, e)}
      onDoubleClick={() => onOpen(video, false)}
    >
      <div className={`card-thumb ${portrait ? "portrait" : ""}`} style={{ aspectRatio: ratio }} onClick={() => onOpen(video, false)}>
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
      <div className="card-body" onClick={() => onOpen(video, false)}>
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
        <div className="card-sub">{relativeTime(video.last_watched_at || video.added_at)}</div>
      </div>
    </article>
  );
}
