import { useCallback, useEffect, useState } from "react";
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

type DrawerMode = "dock" | "wide" | "rail";

const MODE_KEY = "vv-detail-mode";
const WIDTH_KEY = "vv-detail-width";
const MIN_W = 320;
const MAX_W = 780;

function readMode(): DrawerMode {
  const v = localStorage.getItem(MODE_KEY);
  return v === "wide" || v === "rail" || v === "dock" ? v : "dock";
}

function readWidth(): number {
  const n = Number(localStorage.getItem(WIDTH_KEY));
  return Number.isFinite(n) && n >= MIN_W && n <= MAX_W ? n : 384;
}

export function DetailDrawer({ video, tags, collections, onClose, onPlay, onChanged, onError }: Props) {
  const [note, setNote] = useState(video.note);
  const [rating, setRating] = useState(video.rating);
  const [favorite, setFavorite] = useState(video.favorite);
  const [completed, setCompleted] = useState(video.completed);
  const [addedTag, setAddedTag] = useState("");
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<DrawerMode>(readMode);
  const [width, setWidth] = useState<number>(readWidth);
  const [resizing, setResizing] = useState(false);
  const [vw, setVw] = useState(() => window.innerWidth);

  useEffect(() => {
    setNote(video.note);
    setRating(video.rating);
    setFavorite(video.favorite);
    setCompleted(video.completed);
  }, [video.id, video.note, video.rating, video.favorite, video.completed]);

  useEffect(() => { localStorage.setItem(MODE_KEY, mode); }, [mode]);
  useEffect(() => { if (mode !== "rail") localStorage.setItem(WIDTH_KEY, String(Math.round(width))); }, [width, mode]);
  useEffect(() => {
    const onResize = () => setVw(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // 窗口变窄时给主区留出至少 320px，避免详情把网格挤成一条
  const widthCap = Math.min(MAX_W, Math.max(MIN_W, vw - 236 - 320));
  const shownWidth = Math.min(width, widthCap);

  // 抽屉占位后主区会重排，当前卡片可能被挤出视野：重新对齐到可视区
  useEffect(() => {
    const t = window.setTimeout(() => {
      const card = document.querySelector<HTMLElement>(".video-card.selected");
      card?.scrollIntoView({ block: "nearest", inline: "nearest" });
    }, 60);
    return () => window.clearTimeout(t);
  }, [video.id, mode, shownWidth]);

  // 拖左边缘改宽度：抽屉贴右，宽度 = 视口右边界 - 指针 x
  const startResize = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const target = e.currentTarget;
    target.setPointerCapture(e.pointerId);
    setResizing(true);
    const move = (ev: PointerEvent) => {
      const cap = Math.min(MAX_W, Math.max(MIN_W, window.innerWidth - 236 - 320));
      const next = Math.min(cap, Math.max(MIN_W, window.innerWidth - ev.clientX));
      setWidth(next);
    };
    const up = () => {
      setResizing(false);
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", up);
      target.removeEventListener("pointercancel", up);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
    target.addEventListener("pointercancel", up);
  }, []);

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

  // 折叠态：只留一条窄栏，主区几乎全给视频网格
  if (mode === "rail") {
    return (
      <aside className="detail-drawer rail">
        <button className="rail-btn" onClick={() => setMode("dock")} title="展开详情">
          <Icon name="left" size={16} />
        </button>
        <div className="rail-thumb" onClick={() => setMode("dock")} title={video.file_name}>
          {cover ? <img src={cover} alt="" /> : <Icon name="play" size={18} />}
        </div>
        <button className={`rail-btn ${favorite ? "on" : ""}`} title={favorite ? "取消收藏" : "收藏"}
          onClick={() => { setFavorite(!favorite); void patch({ favorite: !favorite }); }}>
          <Icon name="heart" size={16} />
        </button>
        <button className="rail-btn" title="播放" onClick={() => onPlay(video)}>
          <Icon name="play" size={16} />
        </button>
        <div className="rail-spacer" />
        <button className="rail-btn" title="关闭详情" onClick={onClose}>
          <Icon name="x" size={16} />
        </button>
      </aside>
    );
  }

  return (
    <aside
      className={`detail-drawer ${mode} ${resizing ? "resizing" : ""}`}
      style={{ width: shownWidth }}
    >
      <div
        className="detail-resizer"
        onPointerDown={startResize}
        onDoubleClick={() => setWidth(384)}
        title="拖动调宽度，双击复位"
      />
      <div className="detail-head">
        <span className="detail-kicker">视频详情</span>
        <div className="detail-head-actions">
          <button
            className="icon-btn"
            onClick={() => setMode(mode === "wide" ? "dock" : "wide")}
            title={mode === "wide" ? "收窄详情" : "加宽详情（便于编辑备注）"}
          >
            <Icon name={mode === "wide" ? "right" : "left"} size={17} />
          </button>
          <button className="icon-btn" onClick={() => setMode("rail")} title="折叠详情">
            <Icon name="menu" size={17} />
          </button>
          <button className="icon-btn" onClick={onClose} title="关闭（Esc）"><Icon name="x" size={18} /></button>
        </div>
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
            rows={mode === "wide" ? 10 : 5}
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
