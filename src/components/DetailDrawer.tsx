import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Icon } from "./Icons";
import { api } from "../api";
import { formatBytes, formatDate, formatDuration, relativeTime, thumbRatio } from "../format";
import type { Collection, Tag, VideoItem } from "../types";

interface Props {
  video: VideoItem;
  tags: Tag[];
  collections: Collection[];
  onClose: () => void;
  onPlay: (v: VideoItem) => void;
  onChanged: () => void;
  onError: (msg: string) => void;
}

export function DetailDrawer({ video, tags, collections, onClose, onPlay, onChanged, onError }: Props) {
  const [note, setNote] = useState(video.note);
  const [rating, setRating] = useState(video.rating);
  const [favorite, setFavorite] = useState(video.favorite);
  const [completed, setCompleted] = useState(video.completed);
  const [addedTag, setAddedTag] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setNote(video.note);
    setRating(video.rating);
    setFavorite(video.favorite);
    setCompleted(video.completed);
  }, [video.id, video.note, video.rating, video.favorite, video.completed]);

  const patch = async (p: Partial<{ note: string; rating: number; favorite: boolean; completed: boolean }>) => {
    try {
      await api.patch({ id: video.id, ...p });
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  };

  const toggleTag = async (tagId: number, add: boolean) => {
    try {
      await api.tagVideo(video.id, tagId, add);
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  };

  const makeSheet = async () => {
    setBusy(true);
    try {
      await api.contactSheet(video.id);
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const cover = video.contact_sheet ? convertFileSrc(video.contact_sheet) : "";
  const coverRatio = thumbRatio(video.width, video.height);
  const watchedPercent = video.duration_sec > 0
    ? Math.min(100, (video.progress_sec / video.duration_sec) * 100)
    : 0;

  return (
    <aside className="detail-drawer">
      <div className="detail-head">
        <span className="detail-kicker">视频详情</span>
        <button className="icon-btn" onClick={onClose} title="关闭"><Icon name="x" size={18} /></button>
      </div>
      <div className="detail-scroll">
        <div className="detail-cover" style={{ aspectRatio: coverRatio }} onClick={() => onPlay(video)}>
          {cover ? <img src={cover} alt="" /> : (
            <div className="detail-cover-empty"><Icon name="play" size={34} /><span>点击播放</span></div>
          )}
          <button className="detail-play"><Icon name="play" size={16} /> 播放</button>
        </div>

        <h2 className="detail-title">{video.file_name}</h2>
        <div className="detail-path" title={video.path}>{video.path}</div>

        {watchedPercent > 0 && (
          <div className="detail-progress">
            <div className="detail-progress-label">
              <span>观看进度 {formatDuration(video.progress_sec)} / {formatDuration(video.duration_sec)}</span>
              <span>{watchedPercent.toFixed(0)}%</span>
            </div>
            <div className="bar"><span style={{ width: `${watchedPercent}%` }} /></div>
          </div>
        )}

        <div className="detail-actions">
          <button className="btn primary" onClick={() => onPlay(video)}><Icon name="play" size={15} />播放</button>
          <button className="btn" onClick={async () => { try { await api.playExternal(video.id); } catch (e) { onError(String(e)); } }} title="用本机 VLC / mpv 打开">
            <Icon name="external" size={15} />外部播放
          </button>
          <button className="btn" onClick={makeSheet} disabled={busy} title="用 ffmpeg 抽取 4×4 预览图">
            <Icon name="camera" size={15} />{busy ? "生成中…" : "预览图"}
          </button>
        </div>

        <div className="detail-section">
          <div className="section-title">评分与状态</div>
          <div className="rating-row">
            {[1, 2, 3, 4, 5].map((n) => (
              <button
                key={n}
                className={`star-btn ${n <= rating ? "on" : ""}`}
                onClick={() => { setRating(n); void patch({ rating: n }); }}
                title={`${n} 星`}
              >
                <Icon name="star" size={20} />
              </button>
            ))}
            <button className={`toggle-chip ${favorite ? "on" : ""}`} onClick={() => { setFavorite(!favorite); void patch({ favorite: !favorite }); }}>
              <Icon name="heart" size={15} />{favorite ? "已收藏" : "收藏"}
            </button>
            <button className={`toggle-chip ${completed ? "on" : ""}`} onClick={() => { setCompleted(!completed); void patch({ completed: !completed }); }}>
              <Icon name="check" size={15} />{completed ? "已看完" : "标记看完"}
            </button>
          </div>
        </div>

        <div className="detail-section">
          <div className="section-title">标签</div>
          <div className="tag-pills">
            {video.tags.length === 0 && <span className="muted">还没有标签</span>}
            {video.tags.map((name) => {
              const tag = tags.find((t) => t.name === name);
              return (
                <button key={name} className="pill" onClick={() => tag && void toggleTag(tag.id, false)} title="点击移除">
                  {name}<Icon name="x" size={12} />
                </button>
              );
            })}
          </div>
          <select
            className="select full"
            value={addedTag}
            onChange={(e) => {
              const name = e.target.value;
              setAddedTag("");
              if (name) {
                const tag = tags.find((t) => t.name === name);
                if (tag) void toggleTag(tag.id, true);
              }
            }}
          >
            <option value="">+ 添加标签…</option>
            {tags.filter((t) => !video.tags.includes(t.name)).map((t) => (
              <option key={t.id} value={t.name}>{t.category ? `${t.category} / ` : ""}{t.name}</option>
            ))}
          </select>
        </div>

        <div className="detail-section">
          <div className="section-title">目录</div>
          <div className="tag-pills">
            {collections.map((c) => {
              const has = video.collections.includes(c.name);
              return (
                <button
                  key={c.id}
                  className={`pill ${has ? "on" : ""}`}
                  title={has ? "点击移出" : "点击加入"}
                  onClick={async () => {
                    try { await api.setCollection(c.name, video.id, !has); onChanged(); }
                    catch (e) { onError(String(e)); }
                  }}
                >
                  <Icon name="folder" size={12} />{c.name}{has ? <Icon name="check" size={11} /> : null}
                </button>
              );
            })}
          </div>
        </div>

        <div className="detail-section">
          <div className="section-title">备注</div>
          <textarea
            className="note-area"
            value={note}
            onChange={(e) => setNote(e.target.value)}
            onBlur={() => { if (note !== video.note) void patch({ note }); }}
            placeholder="写点什么…剧情、演员、想看的理由"
            rows={5}
          />
        </div>

        <div className="detail-section">
          <div className="section-title">文件信息</div>
          <dl className="fact-list">
            <div><dt>容器</dt><dd>{video.container.toUpperCase() || "—"}</dd></div>
            <div><dt>大小</dt><dd>{formatBytes(video.size_bytes)}</dd></div>
            <div><dt>时长</dt><dd>{formatDuration(video.duration_sec)}</dd></div>
            <div><dt>分辨率</dt><dd>{video.width ? `${video.width}×${video.height}` : "—"}</dd></div>
            <div><dt>视频编码</dt><dd>{video.codec_video || "—"}</dd></div>
            <div><dt>音频编码</dt><dd>{video.codec_audio || "—"}</dd></div>
            <div><dt>观看次数</dt><dd>{video.watch_count}</dd></div>
            <div><dt>上次观看</dt><dd>{relativeTime(video.last_watched_at)}</dd></div>
            <div><dt>加入时间</dt><dd>{formatDate(video.added_at)}</dd></div>
            {video.entry_index > 0 && <div><dt>压缩包内</dt><dd>第 {video.entry_index + 1} / {video.entry_count} 个</dd></div>}
          </dl>
        </div>

        <div className="detail-section">
          <button
            className={`btn danger full ${video.trashed ? "" : ""}`}
            onClick={async () => {
              try { await api.trash(video.id, !video.trashed); onChanged(); onClose(); }
              catch (e) { onError(String(e)); }
            }}
          >
            <Icon name="trash" size={15} />{video.trashed ? "从回收站恢复" : "移入回收站"}
          </button>
          <p className="hint">只从库中移除记录，不会删除磁盘上的视频文件。</p>
        </div>
      </div>
    </aside>
  );
}
