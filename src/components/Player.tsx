import { useCallback, useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Icon } from "./Icons";
import { api } from "../api";
import { formatDuration } from "../format";
import type { PlaybackInfo } from "../types";

interface Props {
  info: PlaybackInfo;
  onClose: () => void;
  onNext: () => void;
  onPrev: () => void;
  onError: (msg: string) => void;
}

const SPEEDS = [0.75, 1, 1.25, 1.5, 2];

export function Player({ info, onClose, onNext, onPrev, onError }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [playing, setPlaying] = useState(false);
  const [current, setCurrent] = useState(0);
  const [duration, setDuration] = useState(info.video.duration_sec);
  const [speed, setSpeed] = useState(1);
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(0.8);
  const [controls, setControls] = useState(true);
  const lastSave = useRef(0);
  const hideTimer = useRef<number | undefined>(undefined);
  const isMpv = info.source === "mpv";

  const save = useCallback((force = false) => {
    const v = videoRef.current;
    const sec = isMpv ? current : v?.currentTime;
    if (!Number.isFinite(sec)) return;
    const now = Date.now();
    if (!force && now - lastSave.current < 5000) return;
    lastSave.current = now;
    void api
      .watch({ id: info.video.id, progress_sec: sec as number, duration_sec: duration || info.video.duration_sec })
      .catch((e) => onError(String(e)));
  }, [info.video.id, info.video.duration_sec, duration, isMpv, current, onError]);

  // mpv 模式：进度/时长/暂停由 IPC 事件推送，不依赖 <video> 元素
  useEffect(() => {
    if (!isMpv) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) => {
      return listen("mpv-property", (e) => {
        const p = e.payload as { name: string; data: unknown };
        if (p.name === "time-pos" && typeof p.data === "number") setCurrent(p.data);
        if (p.name === "duration" && typeof p.data === "number") setDuration(p.data);
        if (p.name === "pause") setPlaying(p.data !== true);
      });
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; }).catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, [isMpv]);

  useEffect(() => {
    if (!isMpv) return;
    const timer = window.setInterval(() => {
      void api.mpvCommand("get_time").catch(() => {});
    }, 5000);
    return () => window.clearInterval(timer);
  }, [isMpv]);

  useEffect(() => {
    const v = videoRef.current;
    if (!v || isMpv) return;
    if (info.start_sec > 0) {
      const onMeta = () => {
        if (Number.isFinite(v.duration)) {
          v.currentTime = Math.min(info.start_sec, Math.max(0, v.duration - 1));
        }
        v.play().catch(() => setPlaying(false));
      };
      v.addEventListener("loadedmetadata", onMeta, { once: true });
      return () => v.removeEventListener("loadedmetadata", onMeta);
    }
    void v.play().catch(() => setPlaying(false));
  }, [info.start_sec, info.media_path, isMpv]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const v = videoRef.current;
      if (e.key === "Escape") { save(true); onClose(); return; }
      if (isMpv) {
        if (e.key === " ") { e.preventDefault(); void api.mpvCommand("toggle"); }
        else if (e.key === "ArrowRight") void api.mpvCommand("seek_relative", 5);
        else if (e.key === "ArrowLeft") void api.mpvCommand("seek_relative", -5);
        else if (e.key.toLowerCase() === "f") void wrapRef.current?.requestFullscreen?.();
        return;
      }
      if (!v) return;
      if (e.key === " ") {
        e.preventDefault();
        if (v.paused) void v.play(); else v.pause();
      } else if (e.key === "ArrowRight") {
        v.currentTime = Math.min(v.duration || 0, v.currentTime + 5);
      } else if (e.key === "ArrowLeft") {
        v.currentTime = Math.max(0, v.currentTime - 5);
      } else if (e.key === "ArrowUp") {
        v.volume = Math.min(1, v.volume + 0.1); setVolume(v.volume);
      } else if (e.key === "ArrowDown") {
        v.volume = Math.max(0, v.volume - 0.1); setVolume(v.volume);
      } else if (e.key.toLowerCase() === "f") {
        void wrapRef.current?.requestFullscreen?.();
      } else if (e.key.toLowerCase() === "c") {
        setControls((c) => !c);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, save, isMpv]);

  useEffect(() => () => save(true), [save]);

  const poke = () => {
    setControls(true);
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      if (playing) setControls(false);
    }, 2600);
  };

  const seek = (e: MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const sec = ((e.clientX - rect.left) / rect.width) * duration;
    if (isMpv) { void api.mpvCommand("seek", sec); return; }
    const v = videoRef.current;
    if (!v || !Number.isFinite(v.duration)) return;
    v.currentTime = sec;
  };

  const snapshot = () => {
    if (isMpv) {
      const name = `${info.video.file_name.replace(/[\\/:*?"<>|]/g, "_")}-${Math.floor(current)}s.png`;
      void api.mpvCommand("screenshot", name).catch((e) => onError(String(e)));
      return;
    }
    const v = videoRef.current;
    if (!v) return;
    const canvas = document.createElement("canvas");
    canvas.width = v.videoWidth;
    canvas.height = v.videoHeight;
    canvas.getContext("2d")?.drawImage(v, 0, 0);
    const a = document.createElement("a");
    a.href = canvas.toDataURL("image/png");
    a.download = `${info.video.file_name.replace(/[\\/:*?"<>|]/g, "_")}-${Math.floor(v.currentTime)}s.png`;
    a.click();
  };

  const external = info.source === "external" || info.source === "missing";
  const controlsVisible = !external && (isMpv || controls);

  return (
    <div className="player-overlay" ref={wrapRef} onMouseMove={poke}>
      <div className="player-stage" onClick={poke}>
        {isMpv ? (
          <div className="player-external">
            <Icon name="play" size={38} />
            <h2>mpv 播放器已接管</h2>
            <p>视频在 mpv 窗口中播放。下方控制条会同步进度、倍速与音量，进度自动写回影匣。</p>
            <span className="muted">按 Esc 返回 · 空格暂停/继续 · ←→ 快退快进</span>
          </div>
        ) : external ? (
          <div className="player-external">
            <Icon name="info" size={38} />
            <h2>这个格式需要外部播放器</h2>
            <p>{info.player_hint || `${info.video.container.toUpperCase()} 暂不能由内置播放器解码，影匣已记住播放位置。`}</p>
            <button className="btn primary" onClick={async () => {
              try { await api.playExternal(info.video.id); }
              catch (e) { onError(String(e)); }
            }}><Icon name="external" size={16} /> 用 VLC / mpv 从上次位置播放</button>
          </div>
        ) : (
          <video
            ref={videoRef}
            src={convertFileSrc(info.media_path)}
            playsInline
            onTimeUpdate={(e) => setCurrent(e.currentTarget.currentTime)}
            onLoadedMetadata={(e) => setDuration(e.currentTarget.duration || 0)}
            onPlay={() => { setPlaying(true); poke(); }}
            onPause={() => { setPlaying(false); setControls(true); save(true); }}
            onEnded={() => {
              void api.watch({ id: info.video.id, progress_sec: duration, duration_sec: duration, completed: true });
              onNext();
            }}
            onError={(e) => onError(`播放失败：${e.currentTarget.error?.message || "未知错误"}`)}
            onClick={(e) => {
              const v = e.currentTarget;
              if (v.paused) void v.play(); else v.pause();
            }}
          />
        )}
      </div>

      <div className="player-top">
        <button className="player-round" onClick={() => { save(true); onClose(); }} title="返回 (Esc)">
          <Icon name="left" size={20} />
        </button>
        <div className="player-heading">
          <div className="truncate">{info.video.file_name}</div>
          <div className="player-sub">
            {info.source === "extracted" ? "压缩包内直接播放（已解出单个文件）" : "本地文件"}
            {info.video.watch_count > 0 ? ` · 第 ${info.video.watch_count + 1} 次观看` : ""}
          </div>
        </div>
      </div>

      {controlsVisible && (
        <div className={`player-controls ${controls || isMpv ? "" : "hidden"}`}>
          <div className="seek" onClick={seek}>
            <span className="seek-fill" style={{ width: `${duration ? (current / duration) * 100 : 0}%` }} />
            <span className="seek-knob" style={{ left: `${duration ? (current / duration) * 100 : 0}%` }} />
          </div>
          <div className="control-row">
            <button className="player-round" onClick={onPrev} title="上一条"><Icon name="back" size={18} /></button>
            <button className="player-round primary" onClick={() => {
              if (isMpv) { void api.mpvCommand("toggle"); return; }
              const v = videoRef.current;
              if (!v) return;
              if (v.paused) void v.play(); else v.pause();
            }} title="播放/暂停 (空格)">
              <Icon name={playing ? "pause" : "play"} size={20} />
            </button>
            <button className="player-round" onClick={onNext} title="下一条"><Icon name="forward" size={18} /></button>
            <span className="time-label">{formatDuration(current)} <span>/</span> {formatDuration(duration)}</span>
            <div className="control-spacer" />
            <button className="player-round" onClick={() => {
              const next = SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.length];
              setSpeed(next);
              if (isMpv) { void api.mpvCommand("speed", next); return; }
              const v = videoRef.current; if (v) v.playbackRate = next;
            }} title="倍速">{speed}×</button>
            <button className="player-round" onClick={() => {
              if (isMpv) {
                const next = !muted; setMuted(next);
                void api.mpvCommand("volume", next ? 0 : volume * 100);
                return;
              }
              const v = videoRef.current; if (!v) return;
              v.muted = !v.muted; setMuted(v.muted);
            }} title="静音"><Icon name="volume" size={18} style={{ opacity: muted ? 0.4 : 1 }} /></button>
            <input
              className="volume-range" type="range" min="0" max="1" step="0.05"
              value={muted ? 0 : volume}
              onChange={(e) => {
                const val = Number(e.target.value);
                setVolume(val);
                setMuted(val === 0);
                if (isMpv) { void api.mpvCommand("volume", val * 100); return; }
                if (videoRef.current) { videoRef.current.volume = val; videoRef.current.muted = val === 0; }
              }}
            />
            <button className="player-round" onClick={snapshot} title="截图"><Icon name="camera" size={18} /></button>
            <button className="player-round" onClick={() => void wrapRef.current?.requestFullscreen?.()} title="全屏 (F)">
              <Icon name="fullscreen" size={18} />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
