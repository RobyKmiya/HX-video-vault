#!/usr/bin/env node
/**
 * Wallpaper Engine 导出助手（本地服务）
 *
 * 住在 127.0.0.1:51789，只服务本机：
 *   GET  /ping     心跳
 *   GET  /health   自检（版本、影匣落点、库根）
 *   POST /probe    收 UI 注入脚本发来的结构探测数据，落盘到 probes/
 *   POST /export   核心：找到壁纸源地址文件夹 → 导出文件夹内的视频文件 → 落到影匣数据目录
 *   POST /log      收前端日志，便于排查
 *
 * 之所以要它：WE 的 UI 是 CEF 渲染的，JS 没有文件系统权限，
 * 真正复制文件这件事只能交给这个外部进程。
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import http from 'node:http';
import { spawn } from 'node:child_process';
import {
  resolveVaultRoot, planIntoVault, copyIntoVault, importIntoVault,
  syncVaultInBackground, isVaultVideo, listBuckets,
} from './vault.mjs';

const PORT = 51789;
const ROOT = import.meta.dirname;
const LOCAL = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
const DATA = path.join(LOCAL, '影匣', 'we-export');
const PROBE_DIR = path.join(DATA, 'probes');
const LOG_DIR = path.join(DATA, 'logs');
const CONFIG_PATH = path.join(ROOT, 'config.json');

const DEFAULT_CONFIG = {
  library: 'D:/Steam/steamapps/common/wallpaper_engine/projects/myprojects',
  steamLibrary: 'D:/Steam/steamapps/workshop/content/431960',
  weRoot: 'D:/Steam/steamapps/common/wallpaper_engine',
  projectsRoot: 'D:/Steam/steamapps/common/wallpaper_engine/projects',
  vaultLibrary: '',            // 影匣数据目录；留空则读桌面端 settings.json
  vaultctl: '',                // vaultctl.exe 路径；留空则自动探测
  nameFrom: 'title',           // title | folder
  includePreview: true,
  autoScan: true,              // 导出后后台同步影匣目录集合
};

function loadConfig() {
  let cfg = { ...DEFAULT_CONFIG };
  try { Object.assign(cfg, JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8'))); } catch { /* 用默认 */ }
  return cfg;
}

function ensureDir(p) { fs.mkdirSync(p, { recursive: true }); }

function logLine(msg) {
  const line = `[${new Date().toISOString()}] ${msg}`;
  try {
    ensureDir(LOG_DIR);
    fs.appendFileSync(path.join(LOG_DIR, `${new Date().toISOString().slice(0, 10)}.log`), line + '\n', 'utf8');
  } catch { /* ignore */ }
  // stdout 可能是已断开的管道（被父进程回收时），写失败绝不能把服务带崩
  try { process.stdout.write(line + '\n'); } catch { /* ignore */ }
}

function readBody(req, limit = 8 * 1024 * 1024) {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks = [];
    req.on('data', (c) => {
      size += c.length;
      if (size > limit) { reject(new Error('body too large')); req.destroy(); return; }
      chunks.push(c);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

function json(res, code, obj) {
  const body = JSON.stringify(obj);
  res.writeHead(code, {
    'Content-Type': 'application/json; charset=utf-8',
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Headers': '*',
    'Access-Control-Allow-Methods': 'GET,POST,OPTIONS',
    'Content-Length': Buffer.byteLength(body),
  });
  res.end(body);
}

function notify(title, text) {
  try {
    const ps1 = path.join(ROOT, 'bin', 'notify.ps1');
    if (!fs.existsSync(ps1)) return;
    const payload = path.join(DATA, 'last-notify.json');
    ensureDir(DATA);
    fs.writeFileSync(payload, JSON.stringify({ title, text }, null, 2), 'utf8');
    spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
      '-WindowStyle', 'Hidden', '-File', ps1, '-ResultPath', payload], { windowsHide: true, stdio: 'ignore' });
  } catch (e) { logLine('通知失败: ' + e.message); }
}

function safeName(s, fallback) {
  const cleaned = String(s || '')
    .replace(/[\\/:*?"<>|]/g, '_')
    .replace(/[\r\n\t]/g, ' ')
    .trim()
    .slice(0, 80);
  return cleaned || fallback;
}

function humanSize(n) {
  if (n > 1 << 30) return (n / (1 << 30)).toFixed(2) + ' GB';
  if (n > 1 << 20) return (n / (1 << 20)).toFixed(1) + ' MB';
  return (n / 1024).toFixed(0) + ' KB';
}

/* ======================================================================
 *  一、定位「壁纸源地址文件夹」
 * ==================================================================== */

/** 宽松的视频扩展名：枚举源文件夹里的视频时用（比影匣入库白名单宽一点） */
const VIDEO_EXT = new Set([
  '.mp4', '.webm', '.mkv', '.mov', '.avi', '.m4v', '.wmv', '.flv',
  '.mpg', '.mpeg', '.ts', '.m2ts', '.ogv', '.3gp', '.vob', '.rmvb', '.rm',
  '.mts', '.m2v', '.asf', '.f4v',
]);

function isVideoFile(p) {
  return VIDEO_EXT.has(path.extname(String(p || '')).toLowerCase());
}

function statSafe(p) {
  try { return fs.statSync(p); } catch { return null; }
}

/** 从某层向上找带 project.json 的那一层（壁纸项目根） */
function findProjectRoot(dir) {
  let cur = dir;
  for (let i = 0; i < 4; i++) {
    if (fs.existsSync(path.join(cur, 'project.json'))) return cur;
    const up = path.dirname(cur);
    if (up === cur) break;
    cur = up;
  }
  return null;
}

function readProjectTitle(dir) {
  try {
    const j = JSON.parse(fs.readFileSync(path.join(dir, 'project.json'), 'utf8'));
    return { title: j.title || '', type: j.type || '', file: j.file || '' };
  } catch {
    return { title: '', type: '', file: '' };
  }
}

/**
 * 从前端传来的壁纸信息里，定位「壁纸源地址文件夹」。
 * WE 的 file 字段有两种语义：
 *   本地项目  → 'myprojects/<id>_/<视频名>.mp4'（相对 <WE>/projects）
 *   创意工坊  → 'D:/…/workshop/content/431960/<workshopid>/<视频名>.mp4'（绝对路径）
 * 所以这里要多条候选路径逐一验证。
 */
function resolveSourceFolder(item, cfg) {
  const raw = String(item.folder || item.path || item.file || '').trim();
  const wid = String(item.workshopid || '').trim();
  const cands = [];
  const add = (p, why) => { if (p) cands.push({ p: path.normalize(p), why }); };

  // A. 前端直接给了源文件夹
  if (item.folder) add(item.folder, '前端指定的源文件夹');

  // B. 创意工坊：workshopid 是最权威的源目录标识
  if (wid && cfg.steamLibrary) add(path.join(cfg.steamLibrary, wid), '创意工坊 workshopid 目录');

  // C. file 字段本身
  if (raw) {
    if (path.isAbsolute(raw)) {
      add(raw, 'file 绝对路径');
    } else {
      for (const base of [cfg.projectsRoot, cfg.library, cfg.steamLibrary, cfg.weRoot]) {
        if (base) add(path.join(base, raw), `file 相对 ${path.basename(String(base))}`);
      }
      // 'myprojects/<id>_/xxx.mp4' → <library>/<id>_
      const seg = raw.replace(/\\/g, '/').split('/');
      for (let i = 0; i < seg.length - 1; i++) {
        if (seg[i] === 'myprojects' && seg[i + 1] && cfg.library) {
          add(path.join(cfg.library, seg[i + 1]), '本地项目目录');
        }
      }
    }
  }

  for (const c of cands) {
    const st = statSafe(c.p);
    if (!st) continue;
    const dir = st.isDirectory() ? c.p : path.dirname(c.p);
    const proj = findProjectRoot(dir);
    return { dir: proj || dir, how: proj ? c.why : c.why + '（无 project.json）', raw: c.p };
  }

  // D. 兜底：按标题在本地项目库里找
  const title = String(item.title || '').trim();
  if (title && cfg.library) {
    let dirs = [];
    try { dirs = fs.readdirSync(cfg.library, { withFileTypes: true }); } catch { dirs = []; }
    for (const d of dirs) {
      if (!d.isDirectory()) continue;
      const p = path.join(cfg.library, d.name);
      const meta = readProjectTitle(p);
      if (meta.title && meta.title === title) {
        return { dir: p, how: '按标题匹配本地项目', raw: p };
      }
    }
  }

  return null;
}

/** 枚举源文件夹内的全部视频文件（递归，跳过隐藏目录） */
function enumerateVideos(dir, maxDepth = 4) {
  const out = [];
  const walk = (cur, depth) => {
    if (depth > maxDepth) return;
    let entries = [];
    try { entries = fs.readdirSync(cur, { withFileTypes: true }); } catch { return; }
    for (const e of entries) {
      if (e.name.startsWith('.')) continue;
      const p = path.join(cur, e.name);
      if (e.isDirectory()) walk(p, depth + 1);
      else if (e.isFile() && isVideoFile(e.name)) out.push(p);
    }
  };
  walk(dir, 0);
  return out;
}

/* ======================================================================
 *  二、导出：源文件夹 → 视频文件 → 影匣数据目录
 * ==================================================================== */

/** 影匣各区间夹里是否已有同名同大小的文件（避免重复导出堆出 "(2)(3)"） */
function findExistingSame(root, name, size) {
  let entries = [];
  try { entries = fs.readdirSync(root, { withFileTypes: true }); } catch { return null; }
  for (const e of entries) {
    if (!e.isDirectory()) continue;
    if (parseBucketStart(e.name) === null) continue;
    const p = path.join(root, e.name, name);
    const st = statSafe(p);
    if (st && st.isFile() && st.size === size) return p;
  }
  return null;
}

/** 与影匣 importer.rs 的 parse_start 同义：'200-399' → 200 */
function parseBucketStart(name) {
  const s = String(name || '');
  const i = s.indexOf('-');
  if (i < 0) return null;
  const a = s.slice(0, i).trim();
  if (!/^\d+$/.test(a)) return null;
  return Number(a);
}

async function handleExport(payload) {
  const cfg = loadConfig();
  const items = Array.isArray(payload.items) ? payload.items : [];
  if (!items.length) return { ok: false, error: '没有收到壁纸' };

  // 影匣数据目录
  const root = resolveVaultRoot(cfg);
  if (!root) return { ok: false, error: '未配置影匣数据目录（config.json 的 vaultLibrary）' };
  if (!fs.existsSync(root)) return { ok: false, error: `影匣数据目录不存在：${root}` };

  const results = [];
  const sources = [];
  const folders = new Map();   // 同一源文件夹只枚举一次

  for (const it of items) {
    const title = String(it.title || '');
    const resolved = resolveSourceFolder(it, cfg);
    if (!resolved) {
      results.push({ ok: false, skipped: true, title, reason: '找不到壁纸源地址文件夹' });
      continue;
    }
    const key = resolved.dir.toLowerCase();
    let entry = folders.get(key);
    if (!entry) {
      const videos = enumerateVideos(resolved.dir);
      entry = { dir: resolved.dir, how: resolved.how, videos };
      folders.set(key, entry);
      logLine(`源文件夹 ${resolved.dir} （${resolved.how}）→ ${videos.length} 个视频`);
    }
    if (!entry.videos.length) {
      results.push({ ok: false, skipped: true, title, reason: '源文件夹里没有视频文件' });
      continue;
    }
    const meta = readProjectTitle(entry.dir);
    for (const v of entry.videos) {
      sources.push({
        src: v,
        title: meta.title || title || path.basename(v),
        from: entry.dir,
        workshopid: it.workshopid ? String(it.workshopid) : '',
      });
    }
  }

  if (!sources.length) {
    const reasons = [...new Set(results.map((r) => r.reason))].filter(Boolean);
    notify('没有可导出的视频', reasons.slice(0, 2).join('；') || '选中的壁纸源文件夹里没有视频文件');
    logLine(`导出请求无视频可导: ${reasons.join(' / ')}`);
    return { ok: true, exported: 0, skipped: results.length, failed: 0, target: root, results };
  }

  // 影匣不支持的扩展名直接跳过，别拷进去当垃圾
  const supported = [];
  for (const s of sources) {
    if (isVaultVideo(s.src)) supported.push(s);
    else {
      results.push({ ok: false, skipped: true, title: s.title, reason: `影匣不支持的格式：${path.extname(s.src)}` });
    }
  }
  if (!supported.length) {
    notify('没有可导入影匣的视频', '源文件夹里的视频格式影匣不认');
    return { ok: true, exported: 0, skipped: results.length, failed: 0, target: root, results };
  }

  // 幂等：影匣里已经有同名同大小的文件就跳过
  const todo = [];
  for (const s of supported) {
    const size = (statSafe(s.src) || {}).size || 0;
    const dup = findExistingSame(root, path.basename(s.src), size);
    if (dup) {
      results.push({ ok: true, skipped: true, title: s.title, to: dup, size, sizeLabel: humanSize(size), reason: '影匣里已有同名同大小文件，跳过' });
      logLine(`跳过（影匣已存在）: ${dup}`);
    } else {
      todo.push(s);
    }
  }

  if (!todo.length) {
    notify('全部已存在', `影匣里已经有这些视频了`);
    return { ok: true, exported: 0, skipped: results.length, failed: 0, target: root, results };
  }

  // 落到影匣区间夹（分桶规则与影匣 importer.rs 一致）
  const { tasks } = planIntoVault(root, todo.map((s) => s.src));
  const copied = copyIntoVault(tasks);
  const doneFiles = [];

  copied.forEach((c, i) => {
    const s = todo[i];
    if (!c.ok) {
      results.push({ ok: false, title: s.title, error: c.error });
      logLine(`复制失败 ${c.source} -> ${c.target}: ${c.error}`);
      return;
    }
    doneFiles.push(c.target);
    results.push({
      ok: true, title: s.title, to: c.target, bucket: c.bucket,
      size: c.size, sizeLabel: humanSize(c.size), workshopid: s.workshopid,
      from: s.from,
    });
    logLine(`导出 OK: ${c.source} -> ${c.target} (${humanSize(c.size)}) [${c.bucket}]`);
  });

  // 登记入库
  let importInfo = { ok: false, added: 0, errors: [] };
  if (doneFiles.length) {
    importInfo = importIntoVault(cfg, doneFiles);
    logLine(`影匣入库: added=${importInfo.added} errors=${importInfo.errors.length}`);
  }

  // 后台全库 scan，把新文件挂进对应的区间夹集合（collection）
  let scan = null;
  if (cfg.autoScan && doneFiles.length) {
    scan = syncVaultInBackground(cfg);
    if (scan) logLine(`已触发影匣目录同步 (pid ${scan.pid})`);
  }

  const okCount = results.filter((r) => r.ok && !r.skipped).length;
  const skipCount = results.filter((r) => r.skipped).length;
  const failCount = results.filter((r) => !r.ok).length;
  const totalSize = results.filter((r) => r.ok && !r.skipped).reduce((a, b) => a + (b.size || 0), 0);

  const head = okCount ? `已导出 ${okCount} 个视频到影匣（${humanSize(totalSize)}）`
    : (skipCount ? `全部已存在，跳过 ${skipCount} 个` : '导出失败');
  const tail = [skipCount && okCount ? `跳过 ${skipCount}` : '', failCount ? `失败 ${failCount}` : ''].filter(Boolean).join('，');
  notify(head + (tail ? '，' + tail : ''), `位置：${root}`);
  return {
    ok: true, target: root, exported: okCount, skipped: skipCount, failed: failCount,
    imported: importInfo.added || 0, importErrors: importInfo.errors || [],
    scanned: !!scan, results,
  };
}

/* ======================================================================
 *  三、HTTP 服务
 * ==================================================================== */

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${PORT}`);

  if (req.method === 'OPTIONS') {
    res.writeHead(204, {
      'Access-Control-Allow-Origin': '*',
      'Access-Control-Allow-Headers': '*',
      'Access-Control-Allow-Methods': 'GET,POST,OPTIONS',
    });
    res.end();
    return;
  }

  try {
    if (url.pathname === '/ping') return json(res, 200, { ok: true, version: 2, pid: process.pid });

    if (url.pathname === '/health') {
      const cfg = loadConfig();
      const root = resolveVaultRoot(cfg);
      let buckets = [];
      try {
        buckets = listBuckets(root).map((b) => `${b.name}:${b.count}`);
      } catch { /* ignore */ }
      return json(res, 200, {
        ok: true, version: 2, pid: process.pid,
        vaultRoot: root, vaultExists: root ? fs.existsSync(root) : false,
        buckets,
      });
    }

    if (url.pathname === '/probe' || url.pathname === '/log') {
      const raw = await readBody(req);
      ensureDir(PROBE_DIR);
      const file = path.join(PROBE_DIR, `${url.pathname.slice(1)}-${Date.now()}.json`);
      fs.writeFileSync(file, raw, 'utf8');
      logLine(`收到 ${url.pathname} (${raw.length} bytes) -> ${file}`);
      return json(res, 200, { ok: true, saved: file });
    }

    if (url.pathname === '/export') {
      const raw = await readBody(req);
      let payload = {};
      try { payload = JSON.parse(raw || '{}'); } catch { return json(res, 400, { ok: false, error: 'JSON 解析失败' }); }
      logLine(`导出请求: ${(payload.items || []).length} 项`);
      const result = await handleExport(payload);
      return json(res, 200, result);
    }

    return json(res, 404, { ok: false, error: 'not found' });
  } catch (e) {
    logLine(`请求出错 ${url.pathname}: ${e.stack || e.message}`);
    return json(res, 500, { ok: false, error: e.message });
  }
});

// 常驻服务不能因为一个异常就消失
process.on('uncaughtException', (e) => logLine('未捕获异常: ' + (e && e.stack ? e.stack : e)));
process.on('unhandledRejection', (e) => logLine('未处理拒绝: ' + (e && e.stack ? e.stack : e)));
process.stdout.on('error', () => { /* 管道断了就忽略 */ });

server.on('error', (e) => {
  if (e.code === 'EADDRINUSE') {
    logLine(`端口 ${PORT} 已被占用，本实例退出（说明助手已经在跑）`);
    process.exit(0);
  }
  logLine('服务错误: ' + e.message);
  process.exit(1);
});

server.listen(PORT, '127.0.0.1', () => {
  ensureDir(DATA);
  const cfg = loadConfig();
  logLine(`导出助手已启动 v2: http://127.0.0.1:${PORT}  (config: ${CONFIG_PATH})`);
  logLine(`影匣落点: ${resolveVaultRoot(cfg) || '(未配置)'}`);
});
