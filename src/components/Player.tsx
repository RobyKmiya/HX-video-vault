import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, MouseEvent } from "react";
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

const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 3];

/** 画面比例：前四个是「怎么把画面放进窗口」，后面是强制成某个数字比例 */
type AspectKey = "contain" | "cover" | "fill" | "origin" | "16:9" | "4:3" | "1:1" | "21:9" | "9:16";

const ASPECTS: { key: AspectKey; label: string; short: string; hint: string }[] = [
  { key: "contain", label: "全部显示（不裁切）", short: "全部", hint: "整帧可见，多余留黑" },
  { key: "cover", label: "覆盖窗口（裁掉溢出）", short: "覆盖", hint: "铺满窗口，可能裁掉边缘" },
  { key: "fill", label: "填充窗口（会变形）", short: "填充", hint: "强行拉满，比例会失真" },
  { key: "origin", label: "原始尺寸（1:1 像素）", short: "原始", hint: "不放大，超出部分留黑" },
  { key: "16:9", label: "强制 16 : 9", short: "16:9", hint: "宽屏" },
  { key: "4:3", label: "强制 4 : 3", short: "4:3", hint: "老片比例" },
  { key: "1:1", label: "强制 1 : 1", short: "1:1", hint: "方形" },
  { key: "21:9", label: "强制 21 : 9", short: "21:9", hint: "超宽银幕" },
  { key: "9:16", label: "强制 9 : 16", short: "9:16", hint: "竖屏" },
];

const RATIOS: Partial<Record<AspectKey, number>> = {
  "16:9": 16 / 9,
  "4:3": 4 / 3,
  "1:1": 1,
  "21:9": 21 / 9,
  "9:16": 9 / 16,
};

interface PlayerPrefs {
  autoNext: boolean;
  speed: number;
  aspect: AspectKey;
  autoHide: boolean;
  loopOne: boolean;
}

const PREFS_KEY = "video-vault.player.prefs";

function loadPrefs(): PlayerPrefs {
  const fallback: PlayerPrefs = { autoNext: true, speed: 1, aspect: "contain", autoHide: true, loopOne: false };
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return fallback;
    const parsed = JSON.parse(raw) as Partial<PlayerPrefs>;
    return {
      autoNext: parsed.autoNext ?? fallback.autoNext,
      speed: SPEEDS.includes(parsed.speed ?? 1) ? (parsed.speed as number) : 1,
      aspect: ASPECTS.some((a) => a.key === parsed.aspect) ? (parsed.aspect as AspectKey) : "contain",
      autoHide: parsed.autoHide ?? fallback.autoHide,
      loopOne: parsed.loopOne ?? fallback.loopOne,
    };
  } catch {
    return fallback;
  }
}

export function Player({ info, onClose, onNext, onPrev, onError }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);

  const [prefs, setPrefs] = useState<PlayerPrefs>(loadPrefs);
  const [playing, setPlaying] = useState(false);
  const [current, setCurrent] = useState(0);
  const [duration, setDuration] = useState(info.video.duration_sec);
  const [speed, setSpeed] = useState(prefs.speed);
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(0.8);
  const [controls, setControls] = useState(true);
  const [menu, setMenu] = useState<"aspect" | "settings" | null>(null);
  const [viewport, setViewport] = useState({ w: 0, h: 0 });
  const lastSave = useRef(0);
  const hideTimer = useRef<number | undefined>(undefined);
  const resumedFor = useRef<number | null>(null);
  const isMpv = info.source === "mpv";
  const external = info.source === "external" || info.source === "missing";

  const updatePrefs = (patch: Partial<PlayerPrefs>) => {
    setPrefs((prev) => {
      const next = { ...prev, ...patch };
      try { localStorage.setItem(PREFS_KEY, JSON.stringify(next)); } catch { /* 无痕模式忽略 */ }
      return next;
    });
  };

  const aspect = prefs.aspect;
  const aspectLabel = ASPECTS.find((a) => a.key === aspect)?.short ?? "全部";

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

  // 换片时把上一集的现场清干净，否则进度条会先跳一下再回来
  useEffect(() => {
    setCurrent(0);
    setDuration(info.video.duration_sec);
    setPlaying(false);
    setControls(true);
    setMenu(null);
    resumedFor.current = null;
  }, [info.video.id, info.video.duration_sec]);

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

  // 内置播放器：换源后自动起播；有上次进度就跳过去
  useEffect(() => {
    const v = videoRef.current;
    if (!v || isMpv) return;
    if (resumedFor.current === info.video.id) return;
    resumedFor.current = info.video.id;
    v.playbackRate = speed;
    const onMeta = () => {
      if (info.start_sec > 0 && Number.isFinite(v.duration)) {
        v.currentTime = Math.min(info.start_sec, Math.max(0, v.duration - 1));
      }
      void v.play().catch(() => setPlaying(false));
    };
    v.addEventListener("loadedmetadata", onMeta, { once: true });
    void v.play().catch(() => { /* 等 loadedmetadata 再试 */ });
    return () => v.removeEventListener("loadedmetadata", onMeta);
  }, [info.video.id, info.start_sec, info.media_path, isMpv, speed]);

  // 舞台尺寸：比例模式要按可用空间算画面框
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const sync = () => setViewport({ w: el.clientWidth, h: el.clientHeight });
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const frameStyle = useMemo<CSSProperties>(() => {
    const W = viewport.w;
    const H = viewport.h;
    if (!W || !H) return { width: "100%", height: "100%" };
    const fixed = RATIOS[aspect];
    if (fixed) {
      let w = W;
      let h = W / fixed;
      if (h > H) { h = H; w = H * fixed; }
      return { width: w, height: h, objectFit: "fill" };
    }
    if (aspect === "origin") {
      const vw = info.video.width || 1280;
      const vh = info.video.height || 720;
      const scale = Math.min(1, W / vw, H / vh);
      return { width: vw * scale, height: vh * scale, objectFit: "fill" };
    }
    const fit = aspect === "cover" ? "cover" : aspect === "fill" ? "fill" : "contain";
    return { width: W, height: H, objectFit: fit };
  }, [aspect, viewport.w, viewport.h, info.video.width, info.video.height]);

  const togglePlay = useCallback(() => {
    if (isMpv) { void api.mpvCommand("toggle"); return; }
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) void v.play().catch((e) => onError(String(e))); else v.pause();
  }, [isMpv, onError]);

  const seekBy = useCallback((delta: number) => {
    if (isMpv) { void api.mpvCommand("seek_relative", delta); return; }
    const v = videoRef.current;
    if (!v) return;
    v.currentTime = Math.max(0, Math.min(v.duration || 0, v.currentTime + delta));
  }, [isMpv]);

  const changeSpeed = useCallback((next: number) => {
    setSpeed(next);
    if (isMpv) { void api.mpvCommand("speed", next); return; }
    if (videoRef.current) videoRef.current.playbackRate = next;
  }, [isMpv]);

  const setMute = useCallback((next: boolean) => {
    setMuted(next);
    if (isMpv) { void api.mpvCommand("volume", next ? 0 : volume * 100); return; }
    if (videoRef.current) videoRef.current.muted = next;
  }, [isMpv, volume]);

  // 快捷键：←→ 5 秒，J/L 10 秒，↑↓ 音量，空格/K 暂停，N/P 上下一条，M 静音，F 全屏
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT")) return;
      const key = e.key.toLowerCase();
      if (e.key === "Escape") { save(true); if (menu) { setMenu(null); return; } onClose(); return; }
      if (key === "n") { e.preventDefault(); save(true); onNext(); return; }
      if (key === "p") { e.preventDefault(); save(true); onPrev(); return; }
      if (key === " " || key === "k") { e.preventDefault(); togglePlay(); return; }
      if (e.key === "ArrowRight") { e.preventDefault(); seekBy(5); return; }
      if (e.key === "ArrowLeft") { e.preventDefault(); seekBy(-5); return; }
      if (key === "l") { e.preventDefault(); seekBy(10); return; }
      if (key === "j") { e.preventDefault(); seekBy(-10); return; }
      if (e.key === "ArrowUp") { e.preventDefault(); setMute(false); const v = Math.min(1, volume + 0.1); setVolume(v); if (isMpv) void api.mpvCommand("volume", v * 100); else if (videoRef.current) videoRef.current.volume = v; return; }
      if (e.key === "ArrowDown") { e.preventDefault(); const v = Math.max(0, volume - 0.1); setVolume(v); setMuted(v === 0); if (isMpv) void api.mpvCommand("volume", v * 100); else if (videoRef.current) { videoRef.current.volume = v; videoRef.current.muted = v === 0; } return; }
      if (key === "f") { void wrapRef.current?.requestFullscreen?.(); return; }
      if (key === "m") { setMute(!muted); return; }
      if (key === "c") { setControls((c) => !c); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, onNext, onPrev, save, menu, togglePlay, seekBy, setMute, volume, muted, isMpv]);

  useEffect(() => () => save(true), [save]);

  const poke = () => {
    setControls(true);
    window.clearTimeout(hideTimer.current);
    if (!prefs.autoHide) return;
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

  const onEnded = () => {
    void api.watch({ id: info.video.id, progress_sec: duration, duration_sec: duration, completed: true });
    if (prefs.loopOne) {
      const v = videoRef.current;
      if (v) { v.currentTime = 0; void v.play(); }
      return;
    }
    if (prefs.autoNext) { onNext(); return; }
    setPlaying(false);
    setControls(true);
  };

  const controlsVisible = !external && (isMpv || controls);

  return (
    <div className="player-overlay" ref={wrapRef} onMouseMove={poke}>
      <div className="player-stage" onClick={() => { setMenu(null); poke(); }}>
        {isMpv ? (
          <div className="player-external">
            <Icon name="play" size={38} />
            <h2>mpv 播放器已接管</h2>
            <p>视频在 mpv 窗口中播放。下方控制条会同步进度、倍速与音量，进度自动写回影匣。</p>
            <span className="muted">按 Esc 返回 · 空格暂停/继续 · ←→ 快退快进 · N/P 上下一条</span>
            <span className="muted">画面比例请在 mpv 窗口里按 <b>W</b> / <b>E</b> 循环切换</span>
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
          <div className="player-viewport" ref={viewportRef}>
            <video
              ref={videoRef}
              src={convertFileSrc(info.media_path)}
              style={frameStyle}
              playsInline
              onTimeUpdate={(e) => setCurrent(e.currentTarget.currentTime)}
              onLoadedMetadata={(e) => {
                setDuration(e.currentTarget.duration || 0);
                e.currentTarget.playbackRate = speed;
              }}
              onPlay={() => { setPlaying(true); poke(); }}
              onPause={() => { setPlaying(false); setControls(true); save(true); }}
              onEnded={onEnded}
              onError={(e) => onError(`播放失败：${e.currentTarget.error?.message || "未知错误"}`)}
              onClick={(e) => { e.stopPropagation(); togglePlay(); }}
              onDoubleClick={(e) => { e.stopPropagation(); void wrapRef.current?.requestFullscreen?.(); }}
            />
          </div>
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
            {!isMpv && !external ? ` · ${info.video.width || "?"}×${info.video.height || "?"} · ${aspectLabel}` : ""}
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
            <button className="player-round" onClick={() => { save(true); onPrev(); }} title="上一条 (P)"><Icon name="back" size={18} /></button>
            <button className="player-round primary" onClick={togglePlay} title="播放/暂停 (空格)">
              <Icon name={playing ? "pause" : "play"} size={20} />
            </button>
            <button className="player-round" onClick={() => { save(true); onNext(); }} title="下一条 (N)"><Icon name="forward" size={18} /></button>
            <span className="time-label">{formatDuration(current)} <span>/</span> {formatDuration(duration)}</span>
            <div className="control-spacer" />

            <div className="player-pop-anchor">
              <button
                className={`player-round ${menu === "aspect" ? "on" : ""}`}
                onClick={(e) => { e.stopPropagation(); setMenu(menu === "aspect" ? null : "aspect"); }}
                disabled={isMpv || external}
                title={isMpv ? "mpv 模式请在播放器窗口里调比例" : "画面比例"}
              >
                {aspectLabel}
              </button>
              {menu === "aspect" && (
                <div className="player-pop" onClick={(e) => e.stopPropagation()}>
                  <div className="player-pop-title">画面比例</div>
                  {ASPECTS.map((a) => (
                    <button
                      key={a.key}
                      className={`player-pop-item ${aspect === a.key ? "on" : ""}`}
                      onClick={() => { updatePrefs({ aspect: a.key }); setMenu(null); }}
                      title={a.hint}
                    >
                      <span>{a.label}</span>
                      <span className="muted">{a.short}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>

            <button className="player-round" onClick={() => changeSpeed(SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.length])} title="倍速">{speed}×</button>
            <button className="player-round" onClick={() => setMute(!muted)} title="静音 (M)"><Icon name="volume" size={18} style={{ opacity: muted ? 0.4 : 1 }} /></button>
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

            <div className="player-pop-anchor">
              <button
                className={`player-round ${menu === "settings" ? "on" : ""}`}
                onClick={(e) => { e.stopPropagation(); setMenu(menu === "settings" ? null : "settings"); }}
                title="播放设置"
              >
                <Icon name="settings" size={17} />
              </button>
              {menu === "settings" && (
                <div className="player-pop player-pop-settings" onClick={(e) => e.stopPropagation()}>
                  <div className="player-pop-title">播放设置</div>
                  <label className="player-pop-row">
                    <input type="checkbox" checked={prefs.autoNext} onChange={(e) => updatePrefs({ autoNext: e.target.checked })} />
                    <span>播完自动下一条</span>
                  </label>
                  <label className="player-pop-row">
                    <input type="checkbox" checked={prefs.loopOne} onChange={(e) => updatePrefs({ loopOne: e.target.checked })} />
                    <span>单条循环（压过自动连播）</span>
                  </label>
                  <label className="player-pop-row">
                    <input type="checkbox" checked={prefs.autoHide} onChange={(e) => { updatePrefs({ autoHide: e.target.checked }); if (!e.target.checked) setControls(true); }} />
                    <span>播放时自动隐藏控制条</span>
                  </label>
                  <div className="player-pop-row">
                    <span>默认倍速</span>
                    <select value={prefs.speed} onChange={(e) => { const v = Number(e.target.value); updatePrefs({ speed: v }); changeSpeed(v); }}>
                      {SPEEDS.map((s) => <option key={s} value={s}>{s}×</option>)}
                    </select>
                  </div>
                  <div className="player-pop-row">
                    <span>默认比例</span>
                    <select value={prefs.aspect} onChange={(e) => updatePrefs({ aspect: e.target.value as AspectKey })}>
                      {ASPECTS.map((a) => <option key={a.key} value={a.key}>{a.label}</option>)}
                    </select>
                  </div>
                  <p className="player-pop-hint">设置存在本机，下次打开影匣仍然生效。</p>
                </div>
              )}
            </div>

            <button className="player-round" onClick={() => void wrapRef.current?.requestFullscreen?.()} title="全屏 (F)">
              <Icon name="fullscreen" size={18} />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
