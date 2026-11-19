import { Icon } from "./Icons";
import { formatBytes, formatDuration } from "../format";
import type { Stats } from "../types";

interface Props {
  stats: Stats;
  onClose: () => void;
}

export function StatsPanel({ stats, onClose }: Props) {
  const cards = [
    { label: "视频条目", value: stats.video_count.toLocaleString(), icon: "grid" as const },
    { label: "文件 / 压缩包", value: stats.archive_count.toLocaleString(), icon: "folder" as const },
    { label: "占用空间", value: formatBytes(stats.total_bytes), icon: "chart" as const },
    { label: "总时长", value: formatDuration(stats.total_duration_sec), icon: "clock" as const },
    { label: "看过", value: stats.watched_count.toLocaleString(), icon: "eye" as const },
    { label: "已看完", value: stats.completed_count.toLocaleString(), icon: "check" as const },
    { label: "收藏", value: stats.favorite_count.toLocaleString(), icon: "heart" as const },
    { label: "标签", value: stats.tag_count.toLocaleString(), icon: "tag" as const },
  ];
  const maxDay = Math.max(1, ...stats.recent_days.map((d) => d.count));
  const watchedRatio = stats.video_count ? Math.round((stats.watched_count / stats.video_count) * 100) : 0;

  return (
    <section className="stats-panel">
      <div className="panel-head">
        <div>
          <h1>统计面板</h1>
          <p>这个库里到底有什么，一眼看清</p>
        </div>
        <button className="icon-btn" onClick={onClose}><Icon name="x" size={18} /></button>
      </div>
      <div className="stat-grid">
        {cards.map((c) => (
          <div className="stat-card" key={c.label}>
            <span className="stat-icon"><Icon name={c.icon} size={19} /></span>
            <span className="stat-value">{c.value}</span>
            <span className="stat-label">{c.label}</span>
          </div>
        ))}
      </div>
      <div className="stats-columns">
        <div className="panel-card">
          <h2>最近入库</h2>
          <div className="bar-chart">
            {stats.recent_days.length === 0 && <p className="muted">还没有数据，先扫描一次吧</p>}
            {[...stats.recent_days].reverse().map((d) => (
              <div className="bar-col" key={d.day} title={`${d.day} · ${d.count} 条`}>
                <div className="bar-track"><span style={{ height: `${(d.count / maxDay) * 100}%` }} /></div>
                <span className="bar-day">{d.day.slice(5)}</span>
              </div>
            ))}
          </div>
        </div>
        <div className="panel-card">
          <h2>观看覆盖</h2>
          <div className="donut-wrap">
            <div className="donut" style={{ background: `conic-gradient(var(--accent) ${watchedRatio}%, var(--line) 0)` }}>
              <div className="donut-inner"><strong>{watchedRatio}%</strong><span>已接触</span></div>
            </div>
            <div className="donut-legend">
              <div><span className="legend-dot" style={{ background: "var(--accent)" }} />看过 {stats.watched_count}</div>
              <div><span className="legend-dot" style={{ background: "var(--line)" }} />未看 {Math.max(0, stats.video_count - stats.watched_count)}</div>
              <div><span className="legend-dot" style={{ background: "var(--ok)" }} />看完 {stats.completed_count}</div>
            </div>
          </div>
        </div>
      </div>
      <div className="panel-card">
        <h2>标签分布</h2>
        {stats.top_tags.length === 0 ? <p className="muted">暂无标签数据</p> : (
          <div className="tag-cloud">
            {stats.top_tags.map((t) => <span key={t.id} className="cloud-pill">{t.name} <b>{t.count}</b></span>)}
          </div>
        )}
      </div>
    </section>
  );
}
