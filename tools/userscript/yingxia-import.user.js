// ==UserScript==
// @name         影匣 · 网页视频入库
// @namespace    video-vault.local
// @version      0.1.1
// @description  在任意带视频的网页上抓标题/作者/出处，记进影匣并可直接下载；标签、备注、防重复与本地库互通
// @author       workbuddy
// @match        *://*/*
// @grant        GM_xmlhttpRequest
// @grant        GM_download
// @grant        GM_getValue
// @grant        GM_setValue
// @connect      127.0.0.1
// @connect      localhost
// @run-at       document-idle
// @noframes
// ==/UserScript==

(function () {
  "use strict";

  // ── 配置 ────────────────────────────────────────────────
  const CFG_KEY = "vv.bridge";
  const cfg = () => GM_getValue(CFG_KEY, { bridge: "http://127.0.0.1:17890", autoPanel: true });
  const saveCfg = (v) => GM_setValue(CFG_KEY, { ...cfg(), ...v });

  // ── 小工具 ──────────────────────────────────────────────
  const $ = (sel, root = document) => root.querySelector(sel);
  const txt = (s) => (s || "").replace(/\s+/g, " ").trim();
  const meta = (name) =>
    txt($(`meta[property="${name}"]`)?.content || $(`meta[name="${name}"]`)?.content || "");

  const host = location.hostname.replace(/^www\./, "");
  const isHanime = /(^|\.)hanime1\.me$/.test(host);

  /** hanime1 作品页的作品号（/watch?v=123） */
  function hanimeId() {
    const m = location.search.match(/[?&]v=(\d+)/);
    return m ? m[1] : "";
  }
  /** hanime1 的作者与标签都挂在 /search?query=...&type=uploader|genre 链接上 */
  function hanimeAuthor() {
    const a = document.querySelector('a[href*="type=uploader"]');
    return txt(a?.textContent);
  }
  function hanimeTags() {
    const out = new Set();
    document.querySelectorAll('a[href*="type=genre"]').forEach((a) => {
      const t = txt(a.textContent);
      if (t && t.length <= 20) out.add(t);
    });
    return [...out];
  }

  /** 作者/上传者：优先站点特征，其次 meta，最后页面标题里的「- xxx」尾巴 */
  function guessAuthor() {
    if (isHanime) {
      const a = hanimeAuthor();
      if (a) return a;
    }
    const selectors = [
      "#channel-name a", "ytd-channel-name a",                     // YouTube
      ".up-name", ".username", ".author a", "[itemprop=author] a",  // B 站/通用
      'a[href*="/user/"]', 'a[href*="/channel/"]', 'a[href*="/author/"]',
      'a[href*="/member/"]', 'a[href*="/uploader/"]',
    ];
    for (const sel of selectors) {
      const el = $(sel);
      const v = txt(el?.textContent);
      if (v && v.length <= 40) return v;
    }
    return meta("author") || meta("og:site_name") || "";
  }

  /** 当前视频的真实地址：<video> 的 src / <source> / og:video */
  function guessVideoUrl() {
    const v = $("video");
    if (v) {
      const src = v.currentSrc || v.src || $("source", v)?.src;
      if (src && !src.startsWith("blob:")) return src;
      if (src && src.startsWith("blob:")) return src; // blob 交给下载按钮给出提示
    }
    return meta("og:video") || meta("og:video:url") || meta("twitter:player:stream") || "";
  }

  function guessTitle() {
    const t =
      meta("og:title") ||
      txt($("h1")?.textContent) ||
      txt(document.title);
    return t.replace(/\s*[-|–]\s*[^-|–]{1,30}$/, "").trim() || t;
  }

  function sanitizeName(name, url) {
    const clean = (name || "video").replace(/[\\/:*?"<>|\r\n\t]/g, "_").slice(0, 120);
    let ext = "";
    try {
      const p = new URL(url, location.href).pathname;
      const m = p.match(/\.(mp4|webm|mkv|mov|m4v|avi|flv|ts|m3u8)$/i);
      if (m) ext = "." + m[1].toLowerCase();
    } catch (_) { /* 相对地址忽略 */ }
    return clean + (clean.toLowerCase().endsWith(ext) ? "" : ext);
  }

  // ── 与影匣本地桥通信 ────────────────────────────────────
  function api(method, path, body, timeoutMs) {
    return new Promise((resolve, reject) => {
      GM_xmlhttpRequest({
        method,
        url: cfg().bridge.replace(/\/$/, "") + path,
        headers: body ? { "Content-Type": "application/json" } : undefined,
        data: body ? JSON.stringify(body) : undefined,
        timeout: timeoutMs || 8000,
        onload: (r) => {
          try { resolve(JSON.parse(r.responseText || "{}")); }
          catch (e) { reject(new Error("桥返回了非 JSON：" + r.responseText?.slice(0, 80))); }
        },
        onerror: () => reject(new Error("连不上影匣（影匣没开？）")),
        ontimeout: () => reject(new Error("影匣响应超时")),
      });
    });
  }

  // ── 面板 UI ─────────────────────────────────────────────
  let panel, btn, state = { found: null, busy: false };

  function el(tag, cls, text) {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }

  function buildPanel() {
    panel = el("div", "vv-panel");
    panel.innerHTML = `
      <div class="vv-head">
        <span class="vv-dot"></span><b>影匣</b>
        <span class="vv-status" data-role="status">检测中…</span>
        <button class="vv-x" data-role="close" title="收起">×</button>
      </div>
      <label class="vv-row"><span>标题</span><input data-role="title" /></label>
      <label class="vv-row"><span>作者</span><input data-role="author" /></label>
      <label class="vv-row"><span>标签</span><input data-role="tags" placeholder="逗号分隔，例如：MMD, 原神" /></label>
      <label class="vv-row"><span>备注</span><textarea data-role="note" rows="2" placeholder="想记的话"></textarea></label>
      <div class="vv-src" data-role="src"></div>
      <div class="vv-actions">
        <button class="vv-btn primary" data-role="save">记入影匣</button>
        <button class="vv-btn" data-role="download">下载视频</button>
        <button class="vv-btn" data-role="check">查重</button>
        <button class="vv-btn" data-role="dlpage" style="display:none">下载页</button>
      </div>
      <div class="vv-tip" data-role="tip"></div>
      <div class="vv-foot"><a data-role="settings">桥地址</a><span data-role="ver"></span></div>
    `;
    document.documentElement.appendChild(panel);

    const q = (r) => panel.querySelector(`[data-role="${r}"]`);
    q("close").onclick = () => panel.classList.remove("open");
    q("save").onclick = onSave;
    q("download").onclick = onDownload;
    q("check").onclick = () => refreshCheck(true);
    q("dlpage").onclick = () => {
      const id = hanimeId();
      if (id) window.open(`https://hanime1.me/download?v=${id}`, "_blank");
    };
    if (isHanime) q("dlpage").style.display = "";
    q("settings").onclick = () => {
      const v = prompt("影匣本地桥地址（默认 http://127.0.0.1:17890）", cfg().bridge);
      if (v) { saveCfg({ bridge: v.trim() }); ping(); }
    };
    q("ver").textContent = "v" + GM_info.script.version;
    fillFields();
  }

  function fillFields() {
    if (!panel) return;
    const q = (r) => panel.querySelector(`[data-role="${r}"]`);
    q("title").value = q("title").value || guessTitle();
    q("author").value = q("author").value || guessAuthor();
    if (isHanime && !q("tags").value) q("tags").value = hanimeTags().join(", ");
    q("src").textContent = location.href;
    q("src").title = location.href;
  }

  function setStatus(text, kind) {
    const s = panel?.querySelector('[data-role="status"]');
    if (!s) return;
    s.textContent = text;
    s.dataset.kind = kind || "";
  }
  function setTip(text, kind) {
    const t = panel?.querySelector('[data-role="tip"]');
    if (!t) return;
    t.textContent = text || "";
    t.dataset.kind = kind || "";
  }

  async function ping() {
    try {
      const r = await api("GET", "/ping");
      if (!r.ok) throw new Error("桥没就绪");
      const st = await api("GET", "/stats");
      setStatus(`已连上 · 库里 ${st.videos ?? "?"} 条`, "ok");
      return true;
    } catch (e) {
      setStatus("未连上影匣", "bad");
      setTip(String(e.message || e), "bad");
      return false;
    }
  }

  /** 服务端抓标题/作者/标签：比 DOM 选择器稳，站点改版也不怕 */
  async function fillFromServer() {
    const q = (r) => panel.querySelector(`[data-role="${r}"]`);
    try {
      const r = await api("GET", "/meta?url=" + encodeURIComponent(location.href), null, 40000);
      if (!r.ok) return;
      if (r.title && !q("title").value.trim()) q("title").value = r.title;
      if (r.author && !q("author").value.trim()) q("author").value = r.author;
      if (Array.isArray(r.tags) && r.tags.length && !q("tags").value.trim()) {
        q("tags").value = r.tags.join(", ");
      }
      if (Array.isArray(r.tags) && r.tags.length) setTip(`已从页面抓到 ${r.tags.length} 个标签`, "ok");
    } catch (_) { /* 抓不到就用手填，不打扰 */ }
  }

  async function refreshCheck(loud) {
    const url = location.href;
    try {
      const r = await api("GET", "/check?url=" + encodeURIComponent(url));
      if (r.found) {
        state.found = r.video;
        setStatus(`已入库：${r.video.file_name}`, "ok");
        if (loud) setTip(`这条已在库里（第 ${r.video.folder_no} 号夹第 ${r.video.slot_no} 条），可以不再重复下载`, "ok");
      } else {
        state.found = null;
        setStatus("库里还没有这条", "");
        if (loud) setTip("这条还没入库", "");
      }
    } catch (e) {
      if (loud) setTip(String(e.message || e), "bad");
    }
  }

  function payload() {
    const q = (r) => panel.querySelector(`[data-role="${r}"]`);
    return {
      url: location.href,
      title: q("title").value.trim(),
      author: q("author").value.trim(),
      site: host,
      tags: q("tags").value.trim(),
      note: q("note").value.trim(),
    };
  }

  async function onSave() {
    if (state.busy) return;
    state.busy = true;
    setTip("提交中…");
    try {
      const r = await api("POST", "/hook", payload());
      if (!r.ok) throw new Error(r.error || "提交失败");
      if (r.duplicate) {
        setTip(`已记下，但这条出处已经在库里（#${r.duplicate}），不用再下一次`, "ok");
      } else {
        setTip("已记下。把文件下载/拖进影匣后，标签和出处会自动挂上（文件名要含标题）", "ok");
      }
      setStatus("已记录", "ok");
    } catch (e) {
      setTip(String(e.message || e), "bad");
    } finally {
      state.busy = false;
    }
  }

  async function onDownload() {
    // hanime1：页面里是播放器，直链要带 Referer，交给影匣下载器最稳
    if (isHanime) {
      const id = hanimeId();
      if (!id) { setTip("这个页面没有作品号（不是 /watch?v= 页）", "bad"); return; }
      setTip("正在让影匣解析最高画质…");
      try {
        await api("POST", "/hook", payload(), 15000);     // 元数据先记下，下载完成后自动挂上
        const r = await api("POST", "/download", {
          url: location.href, title: payload().title, tags: payload().tags,
        }, 45000);
        if (!r.ok) throw new Error(r.error || "影匣没能接单");
        setTip("已交给影匣：后台解析最高画质并下载，进度在影匣「下载」面板看；下完自动入库并挂标签", "ok");
        setStatus("影匣后台下载中", "ok");
      } catch (e) {
        setTip(String(e.message || e) + "（也可以点下面的「打下载页」手动挑画质）", "bad");
      }
      return;
    }
    const src = guessVideoUrl();
    if (!src) { setTip("这个页面没找到直链视频（可能是 iframe 内播放器）", "bad"); return; }
    if (src.startsWith("blob:")) {
      setTip("这是 blob 流，浏览器脚本抓不到原文件；请用站点的下载按钮，或用影匣的链接下载功能", "bad");
      return;
    }
    if (/\.m3u8(\?|$)/i.test(src)) {
      setTip("这是 m3u8 分片流，脚本下不了整段；先用「记入影匣」记下，再用别的工具合并", "bad");
      return;
    }
    const p = payload();
    // 先把元数据记进去，下载落盘后影匣整理时按标题挂上
    try { await api("POST", "/hook", p); } catch (_) { /* 桥没开也不挡下载 */ }
    const name = sanitizeName(p.title, src);
    setTip(`开始下载：${name}`);
    GM_download({
      url: src,
      name,
      saveAs: false,
      onload: () => setTip(`下载完成：${name}。打开影匣「扫描库」即可入库并自动挂标签`, "ok"),
      onerror: (err) => setTip("下载失败：" + (err?.error || "浏览器拒绝或链接需登录"), "bad"),
      ontimeout: () => setTip("下载超时", "bad"),
    });
  }

  // ── 挂载 ────────────────────────────────────────────────
  function ensureButton() {
    if (btn) return;
    btn = el("button", "vv-fab", "影匣");
    btn.title = "影匣 · 网页视频入库";
    btn.onclick = () => {
      panel.classList.toggle("open");
      if (panel.classList.contains("open")) {
        fillFields();
        void ping().then(() => refreshCheck(false));
        void fillFromServer();
      }
    };
    document.documentElement.appendChild(btn);
  }

  function hasVideo() {
    return !!($("video") || meta("og:video") || meta("og:video:url"));
  }

  function boot() {
    if (!hasVideo()) return;
    ensureButton();
    if (!panel) buildPanel();
    // 站点是 SPA 时地址会变，重新抓一次标题/作者并查重
    let lastHref = location.href;
    setInterval(() => {
      if (location.href === lastHref) return;
      lastHref = location.href;
      if (!panel) return;
      panel.querySelector('[data-role="title"]').value = guessTitle();
      panel.querySelector('[data-role="author"]').value = guessAuthor();
      panel.querySelector('[data-role="src"]').textContent = location.href;
      if (panel.classList.contains("open")) void refreshCheck(false);
    }, 1500);
  }

  // ── 样式 ────────────────────────────────────────────────
  const css = `
  .vv-fab{position:fixed;right:18px;bottom:18px;z-index:2147483000;padding:8px 14px;border-radius:999px;
    border:1px solid #f97316;background:#1c1815;color:#f97316;font:600 13px/1 system-ui,sans-serif;
    cursor:pointer;box-shadow:0 6px 20px rgba(0,0,0,.45)}
  .vv-fab:hover{background:#f97316;color:#1c1815}
  .vv-panel{position:fixed;right:18px;bottom:64px;width:320px;z-index:2147483001;display:none;
    background:#1c1815;color:#f3ece3;border:1px solid #3a2f26;border-radius:14px;padding:12px;
    font:13px/1.5 system-ui,"Microsoft YaHei",sans-serif;box-shadow:0 18px 60px rgba(0,0,0,.6)}
  .vv-panel.open{display:block}
  .vv-head{display:flex;align-items:center;gap:8px;margin-bottom:8px}
  .vv-dot{width:8px;height:8px;border-radius:50%;background:#f97316}
  .vv-status{margin-left:auto;font-size:11px;color:#a89c8e;max-width:150px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .vv-status[data-kind=ok]{color:#7bd88f}
  .vv-status[data-kind=bad]{color:#ff8b96}
  .vv-x{margin-left:6px;background:none;border:0;color:#a89c8e;font-size:16px;cursor:pointer}
  .vv-row{display:flex;gap:8px;align-items:center;margin:6px 0}
  .vv-row>span{width:34px;flex:0 0 34px;color:#a89c8e;font-size:12px}
  .vv-row input,.vv-row textarea{flex:1;min-width:0;background:#14110f;border:1px solid #3a2f26;
    border-radius:8px;color:#f3ece3;padding:6px 8px;font:inherit;resize:vertical}
  .vv-src{font-size:11px;color:#7f7468;margin:6px 0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .vv-actions{display:flex;gap:6px;margin-top:8px}
  .vv-btn{flex:1;padding:7px 0;border-radius:8px;border:1px solid #3a2f26;background:#241f1a;
    color:#f3ece3;font:inherit;cursor:pointer}
  .vv-btn:hover{border-color:#f97316}
  .vv-btn.primary{background:#f97316;border-color:#f97316;color:#1c1815;font-weight:600}
  .vv-tip{margin-top:8px;font-size:11.5px;color:#a89c8e;min-height:16px}
  .vv-tip[data-kind=ok]{color:#7bd88f}
  .vv-tip[data-kind=bad]{color:#ff8b96}
  .vv-foot{display:flex;justify-content:space-between;margin-top:8px;font-size:10.5px;color:#6d6358}
  .vv-foot a{cursor:pointer;text-decoration:underline}
  `;
  const style = document.createElement("style");
  style.textContent = css;
  document.documentElement.appendChild(style);

  // 页面是动态渲染的：等视频元素出现再挂
  if (hasVideo()) boot();
  else {
    const mo = new MutationObserver(() => { if (hasVideo()) { mo.disconnect(); boot(); } });
    mo.observe(document.documentElement, { childList: true, subtree: true });
    setTimeout(() => mo.disconnect(), 20000);
  }
})();
