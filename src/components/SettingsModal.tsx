import { useState, useEffect } from "react";
import type { Dispatch, SetStateAction } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { Icon } from "./Icons";
import { api } from "../api";
import {
  ACTIONS, bindAction, comboFromEvent, saveKeyMap, unbindAction,
  type ActionId, type KeyMap,
} from "../shortcuts";
import {
  GESTURE_LABELS, GESTURE_ACTION_LABELS, saveGestures,
  type GestureAction, type GestureMap, type GestureSlot,
} from "../gestures";
import type { Settings } from "../types";

export function SettingsModal({
  settings, setSettings, keymap, setKeymap, gestures, setGestures, onClose, notify,
}: {
  settings: Settings;
  setSettings: (s: Settings) => void;
  keymap: KeyMap;
  setKeymap: Dispatch<SetStateAction<KeyMap>>;
  gestures: GestureMap;
  setGestures: Dispatch<SetStateAction<GestureMap>>;
  onClose: () => void;
  notify: (m: string) => void;
}) {
  const [draft, setDraft] = useState<Settings>(settings);
  const [recording, setRecording] = useState<ActionId | null>(null);
  /** 录制时的提示（冲突、注册失败…） */
  const [keyHint, setKeyHint] = useState("");
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    // 界面内可自查版本号：更新是否生效一目了然
    getVersion().then(setAppVersion).catch(() => {});
  }, []);
  useEffect(() => {
    // 呼回键的真值在 Rust（注册过的才算数）：打开面板时以 Rust 为准校正一遍，
    // 免得另一处改过之后 localStorage 与实际注册的键对不上。
    void api.getBossRecall()
      .then((binding) => {
        if (!binding) return;
        setKeymap((prev) => {
          if (prev["boss.recall"] === binding) return prev;
          const next = { ...prev, "boss.recall": binding };
          saveKeyMap(next);
          return next;
        });
      })
      .catch(() => {});
  }, [setKeymap]);
  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setRecording(null); setKeyHint(""); return; }
      const combo = comboFromEvent(e);
      if (!combo) return;
      // 冲突提示：同一个组合只能绑一个动作，不覆盖别人的（宁可让人换一个）
      const conflict = (Object.keys(keymap) as ActionId[])
        .find((id) => id !== recording && keymap[id] === combo);
      if (conflict) {
        const label = ACTIONS.find((a) => a.id === conflict)?.label ?? conflict;
        setKeyHint(`「${combo}」已经绑给「${label}」了，换一个`);
        return;
      }
      const previous = keymap;
      const next = bindAction(keymap, recording, combo);
      saveKeyMap(next); setKeymap(next); setRecording(null); setKeyHint("");
      // 呼回键是系统级热键，必须推给 Rust 注册；注册失败就回滚并说明原因
      if (recording === "boss.recall") {
        void api.setBossRecall(combo).catch((error) => {
          setKeyHint(`呼回键「${combo}」注册失败：${String(error)}`);
          saveKeyMap(previous); setKeymap(previous);
        });
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording, keymap, setKeymap]);
  const save = async () => {
    try {
      await api.updateSettings(draft);
      setSettings(draft);
      notify("设置已保存");
      onClose();
    } catch (e) { notify(String(e)); }
  };
  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="panel-head"><div><h1>设置</h1><p>本地配置，不上传{appVersion ? ` · 影匣 v${appVersion}` : ""}</p></div>
          <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button></div>
        <div className="modal-body">
          <div className="field">
            <label>当前库目录</label>
            <div className="field-row">
              <input readOnly value={draft.library_root || "未选择"} />
              <button className="btn" onClick={async () => {
                const dir = await api.pickDirectory();
                if (dir) setDraft((d) => ({ ...d, library_root: dir }));
              }}>选择</button>
            </div>
          </div>
          <div className="field">
            <label>ffmpeg 路径（用于接触表预览）</label>
            <input value={draft.ffmpeg_path || ""} placeholder="留空则自动检测，例如 D:\\MESAVAGE\\Tools\\ffmpeg\\ffmpeg.exe"
              onChange={(e) => setDraft((d) => ({ ...d, ffmpeg_path: e.target.value || null }))} />
          </div>
          <div className="field">
            <label>ffprobe 路径（用于读取时长/编码）</label>
            <input value={draft.ffprobe_path || ""} placeholder="留空则自动检测"
              onChange={(e) => setDraft((d) => ({ ...d, ffprobe_path: e.target.value || null }))} />
          </div>
          <div className="field">
            <label>拖拽导入根目录</label>
            <div className="field-row">
              <input
                value={draft.import_root || ""}
                placeholder={`留空 = 库里自动用 ${draft.library_root ? `${draft.library_root}\\Supervideo` : "<库目录>\\Supervideo"}`}
                onChange={(e) => setDraft((d) => ({ ...d, import_root: e.target.value || null }))}
              />
              <button className="btn" onClick={async () => {
                const dir = await api.pickDirectory();
                if (dir) setDraft((d) => ({ ...d, import_root: dir }));
              }}>选择</button>
              <button className="btn" title="清空 = 用库根下的 Supervideo" onClick={() => setDraft((d) => ({ ...d, import_root: null }))}>默认</button>
            </div>
            <p className="hint">拖视频进来就复制到这个根目录下的编号夹：1、2、3…每夹 200 个，只往后追加、不再回头填空位；满了自动开下一号。</p>
          </div>
          <div className="field">
            <label>播放器内核</label>
            <select value={draft.player_pref} onChange={(e) => setDraft((d) => ({ ...d, player_pref: e.target.value }))}>
              <option value="auto">自动（优先 mpv，其次内置）</option>
              <option value="builtin">内置播放器（mp4/webm）</option>
              <option value="mpv">mpv（IPC · 字幕/倍速/截图）</option>
              <option value="vlc">VLC（冷门格式兜底）</option>
            </select>
          </div>
          <div className="field">
            <label>mpv 可执行文件路径（可选）</label>
            <input value={draft.mpv_path || ""} placeholder="留空则自动探测；例如 D:\\TOOLS\\mpv\\mpv.exe"
              onChange={(e) => setDraft((d) => ({ ...d, mpv_path: e.target.value || null }))} />
          </div>
          <div className="field">
            <label>画廊双击</label>
            <select value={draft.double_click || "play"} onChange={(e) => setDraft((d) => ({ ...d, double_click: e.target.value }))}>
              <option value="play">直接播放（推荐）</option>
              <option value="detail">打开详情抽屉</option>
            </select>
            <p className="hint">单击始终是选中并打开详情；双击走这里的设置。</p>
          </div>
          <div className="field">
            <label>主题</label>
            <div className="field-row">
              <select value={draft.theme || "dark"} onChange={(e) => setDraft((d) => ({ ...d, theme: e.target.value }))}>
                <option value="dark">橙黑（暗色，默认）</option>
                <option value="light">浅色</option>
              </select>
              <label className="theme-color" title="主色：按钮、选中态、进度条都用它">
                <span>主色</span>
                <input
                  type="color"
                  value={/^#[0-9a-fA-F]{6}$/.test(draft.accent || "") ? draft.accent : "#f97316"}
                  onChange={(e) => setDraft((d) => ({ ...d, accent: e.target.value }))}
                />
              </label>
              <button className="btn" onClick={() => setDraft((d) => ({ ...d, accent: "" }))} title="恢复主题默认色">默认色</button>
            </div>
            <p className="hint">主色会影响按钮、选中态、进度条与聚焦高亮。留空 = 用主题自带色（橙黑主题默认 #f97316）。</p>
          </div>
          <div className="field setting-checks">
            <label><input type="checkbox" checked={!!draft.immersive_default} onChange={(e) => setDraft((d) => ({ ...d, immersive_default: e.target.checked }))} />播放默认无边框沉浸</label>
            <label><input type="checkbox" checked={draft.show_card_info !== false} onChange={(e) => setDraft((d) => ({ ...d, show_card_info: e.target.checked }))} />卡片显示文件名与信息</label>
          </div>
          <div className="field keymap-field">
            <label>快捷键（默认全部未绑定，点右侧录制）</label>
            <p className="hint">输入框里打字不会触发快捷键；Esc 取消录制。同一个组合只保留最后一个绑定。老板键两个动作是「应用内按键 + 系统级热键」，呼回那个会真的注册到系统。</p>
            <div className="keymap-list">
              {(["排序", "播放", "选择与收藏", "文件", "界面"] as const).map((group) => (
                <div className="keymap-group" key={group}>
                  <div className="keymap-group-title">{group}</div>
                  {ACTIONS.filter((a) => a.group === group).map((a) => (
                    <div className="keymap-row" key={a.id}>
                      <span title={a.hint || a.label}>{a.label}</span>
                      <button className={`keymap-key ${recording === a.id ? "rec" : ""}`} onClick={() => setRecording(a.id)}>
                        {recording === a.id ? "按下组合键…" : keymap[a.id] || "未绑定（点击录制）"}
                      </button>
                      {keymap[a.id] && <button className="keymap-clear" title="清除绑定" onClick={() => {
                        const next = unbindAction(keymap, a.id);
                        saveKeyMap(next); setKeymap(next); setKeyHint("");
                        // 呼回键清掉之后，Rust 侧恢复默认热键（不然就再也唤不回来了）
                        if (a.id === "boss.recall") void api.setBossRecall("Ctrl+Shift+H").catch(() => {});
                      }}><Icon name="x" size={12} /></button>}
                    </div>
                  ))}
                </div>
              ))}
            </div>
            {keyHint && <p className="hint keymap-hint-warn">{keyHint}</p>}
          </div>
          <div className="field keymap-field">
            <label>鼠标手势（播放器内）</label>
            <p className="hint">单击画面仍是立即播放/暂停（加了延迟会发木），所以这里只管双击和 Ctrl+滚轮。</p>
            <div className="keymap-list">
              {(Object.keys(GESTURE_LABELS) as GestureSlot[]).map((slot) => (
                <div className="keymap-row" key={slot}>
                  <span>{GESTURE_LABELS[slot]}</span>
                  <select
                    className="keymap-select"
                    value={gestures[slot]}
                    onChange={(e) => {
                      const next = { ...gestures, [slot]: e.target.value as GestureAction };
                      saveGestures(next); setGestures(next);
                    }}
                  >
                    {(Object.keys(GESTURE_ACTION_LABELS) as GestureAction[]).map((action) => (
                      <option key={action} value={action}>{GESTURE_ACTION_LABELS[action]}</option>
                    ))}
                  </select>
                </div>
              ))}
            </div>
          </div>
          <p className="hint">修改 ffmpeg 路径后需重启影匣生效。影片文件不会被移动或上传。</p>
        </div>
        <div className="modal-actions">
          <button className="btn" onClick={onClose}>取消</button>
          <button className="btn primary" onClick={() => void save()}>保存</button>
        </div>
      </div>
    </div>
  );
}
