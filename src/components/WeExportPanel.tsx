import { useEffect, useRef, useState } from "react";
import { Icon } from "./Icons";
import { api } from "../api";
import type { WeExportStatus } from "../types";

interface Props {
  onClose: () => void;
  notify: (m: string, kind?: "info" | "success" | "error") => void;
}

const PATH_FIELDS: { key: keyof WeExportStatus["config"]; label: string; hint: string }[] = [
  { key: "vaultLibrary", label: "影匣库目录", hint: "视频落到哪个根目录（区间夹所在）" },
  { key: "weRoot", label: "壁纸引擎根目录", hint: "Wallpaper Engine 安装目录" },
  { key: "library", label: "本地项目目录", hint: "myprojects 所在目录" },
  { key: "projectsRoot", label: "项目根目录", hint: "projects 目录" },
  { key: "steamLibrary", label: "创意工坊目录", hint: "workshop/content/431960" },
];

export function WeExportPanel({ onClose, notify }: Props) {
  const [st, setSt] = useState<WeExportStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [testResult, setTestResult] = useState<string>("");
  const cfgRef = useRef<WeExportStatus["config"]>({});
  const [cfgDirty, setCfgDirty] = useState(false);

  const refresh = () => api.weExportStatus().then(setSt).catch(() => {});
  useEffect(() => {
    refresh();
    const t = window.setInterval(refresh, 4000);
    return () => window.clearInterval(t);
  }, []);

  // 状态到达后，把 config 同步进可编辑副本（仅首次或用户没在编辑时）
  useEffect(() => {
    if (st && !cfgDirty) cfgRef.current = { ...st.config };
  }, [st, cfgDirty]);

  const act = async (fn: () => Promise<unknown>, ok?: string) => {
    setBusy(true);
    try {
      await fn();
      if (ok) notify(ok, "success");
      await refresh();
    } catch (e) {
      notify(String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const doTest = async () => {
    setTestResult("");
    setBusy(true);
    try {
      const r = await api.weExportTest();
      const buckets = Array.isArray((r as { buckets?: unknown[] }).buckets)
        ? (r as { buckets: string[] }).buckets.join("、")
        : "—";
      setTestResult(
        `库根：${(r as { vaultRoot?: string }).vaultRoot ?? "—"}\n区间夹：${buckets}`,
      );
      notify("助手自检通过", "success");
    } catch (e) {
      setTestResult(String(e));
      notify(String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const saveConfig = async () => {
    setBusy(true);
    try {
      const next = cfgRef.current;
      const prev = st?.config ?? {};
      const keys = new Set([...Object.keys(prev), ...Object.keys(next)]);
      for (const k of keys) {
        if (JSON.stringify((prev as Record<string, unknown>)[k]) !== JSON.stringify((next as Record<string, unknown>)[k])) {
          await api.weExportSetConfig(k, (next as Record<string, unknown>)[k]);
        }
      }
      setCfgDirty(false);
      notify("配置已保存，下次导出生效", "success");
      await refresh();
    } catch (e) {
      notify(String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const setCfg = (key: keyof WeExportStatus["config"], value: unknown) => {
    cfgRef.current = { ...cfgRef.current, [key]: value };
    setCfgDirty(true);
  };

  const alive = st?.alive;

  return (
    <section className="stats-panel">
      <div className="panel-head">
        <div>
          <h1>壁纸导出助手</h1>
          <p>在 Wallpaper Engine 里点「导出」，视频会自动落进影匣库并登记入库</p>
        </div>
        <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
      </div>

      <div className="panel-card">
        <div className="we-status-row">
          <span className={`we-badge ${alive ? "on" : ""}`}>
            <span className="we-dot" />{alive ? "运行中" : "已停止"}
          </span>
          <span className="muted">端口 {st?.port ?? 51789}</span>
          {st?.pid ? <span className="muted">PID {st.pid}</span> : null}
          {st?.vault_root ? <span className="muted truncate" title={st.vault_root}>库：{st.vault_root}</span> : null}
        </div>
        <div className="we-status-sub">
          <span className="muted truncate" title={st?.node_path}>node：{st?.node_path || "未找到"}</span>
        </div>
        <div className="we-actions">
          <button className="btn primary small" disabled={busy} onClick={() => act(() => api.weExportStart(), "助手已启动")}>
            <Icon name="play" size={14} />启动
          </button>
          <button className="btn small" disabled={busy} onClick={() => act(() => api.weExportStop(), "助手已停止")}>
            <Icon name="x" size={14} />停止
          </button>
          <button className="btn small" disabled={busy} onClick={() => void doTest()}>
            <Icon name="check" size={14} />自检
          </button>
          <button className="btn small" disabled={busy || !cfgDirty} onClick={() => void saveConfig()}>
            <Icon name="settings" size={14} />保存配置
          </button>
        </div>
        {testResult && <pre className="we-test-result">{testResult}</pre>}
        {!st?.node_path && (
          <div className="warning">没找到 node.exe，助手起不来。装个 Node.js 或在此配置 node 路径。</div>
        )}
      </div>

      <div className="panel-card">
        <h2>配置</h2>
        {PATH_FIELDS.map((f) => (
          <div className="field" key={f.key}>
            <label>{f.label}</label>
            <input
              value={String(cfgRef.current[f.key] ?? "")}
              onChange={(e) => setCfg(f.key, e.target.value)}
              placeholder={f.hint}
            />
            <div className="hint">{f.hint}</div>
          </div>
        ))}
        <div className="field">
          <label>命名依据</label>
          <select value={String(cfgRef.current.nameFrom ?? "title")} onChange={(e) => setCfg("nameFrom", e.target.value)}>
            <option value="title">按壁纸标题</option>
            <option value="folder">按文件夹名</option>
          </select>
        </div>
        <div className="we-toggles">
          <label className="we-toggle">
            <input type="checkbox" checked={!!cfgRef.current.autoScan} onChange={(e) => setCfg("autoScan", e.target.checked)} />
            导出后自动扫描入库
          </label>
          <label className="we-toggle">
            <input type="checkbox" checked={!!cfgRef.current.includePreview} onChange={(e) => setCfg("includePreview", e.target.checked)} />
            附带预览图
          </label>
        </div>
        <div className="field">
          <label>node 路径（可选，留空自动探测）</label>
          <input value={String(cfgRef.current.node ?? "")} onChange={(e) => setCfg("node", e.target.value)} placeholder="C:\Program Files\nodejs\node.exe" />
        </div>
      </div>

      <div className="panel-card">
        <h2>最近日志</h2>
        {!st?.logs?.length ? <p className="muted">暂无日志</p> : (
          <div className="we-logs">{st.logs.map((l, i) => <div key={i} className="we-log-line">{l}</div>)}</div>
        )}
      </div>
    </section>
  );
}
