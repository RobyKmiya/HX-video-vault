import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./App.css";

/** 启动错误边界：渲染崩溃不再黑屏，给出错误信息与重试入口 */
class BootBoundary extends React.Component<{ children: React.ReactNode }, { err: Error | null }> {
  state = { err: null as Error | null };
  static getDerivedStateFromError(err: Error) {
    return { err };
  }
  render() {
    if (this.state.err) {
      return (
        <div
          style={{
            height: "100%", display: "grid", placeItems: "center",
            background: "radial-gradient(120% 120% at 50% 30%, #141926 0%, #0e1118 55%, #0a0d13 100%)",
            color: "#e8edf6", fontFamily: "system-ui",
          }}
        >
          <div style={{ textAlign: "center", maxWidth: 680, padding: 24 }}>
            <div style={{ fontSize: 18, marginBottom: 10 }}>影匣启动失败</div>
            <pre style={{ whiteSpace: "pre-wrap", color: "#98a2b5", fontSize: 12, textAlign: "left" }}>
              {String(this.state.err?.message || this.state.err)}
            </pre>
            <button
              onClick={() => location.reload()}
              style={{
                marginTop: 14, padding: "6px 18px", borderRadius: 8, cursor: "pointer",
                border: "1px solid #39435a", background: "#1b2130", color: "#e8edf6",
              }}
            >
              重试
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <BootBoundary>
      <App />
    </BootBoundary>
  </React.StrictMode>,
);
