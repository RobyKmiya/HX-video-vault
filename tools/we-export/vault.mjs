#!/usr/bin/env node
/**
 * 影匣（VideoVault）落点集成
 *
 * 职责：把「壁纸源文件夹里的视频文件」落到影匣数据目录，并登记入库。
 *
 * 这里的分桶规则必须和影匣本体一致，否则两边对不上：
 *   影匣 E:\VideoVault\src-tauri\src\importer.rs
 *     - CAP = 200
 *     - parse_start(name)     "200-399" -> 200
 *     - pick_target(order, counts, cap)
 *         按区间起点升序找第一个未满夹（漏缺往前补），全满则开新区间
 *     - count_videos(dir)     该目录下一层里 is_video 的文件数
 *     - unique_target_name(dir, source)
 *         原名可用就原样，否则 "stem (2).ext"、"stem (3).ext"…
 *   影匣 E:\VideoVault\src-tauri\src\media.rs
 *     - VIDEO_EXT 扩展名白名单
 */
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync, spawn } from 'node:child_process';

/** 与影匣 media.rs 的 VIDEO_EXT 保持一致（注意：和"壁纸视频"的宽松列表不同） */
const VAULT_VIDEO_EXT = new Set([
  'mp4', 'mkv', 'avi', 'mov', 'wmv', 'flv', 'webm', 'm4v', 'ts', 'm2ts',
  'mpg', 'mpeg', '3gp', 'vob', 'rmvb', 'rm', 'mts', 'm2v', 'asf', 'f4v',
]);

/** 影匣区间夹容量，对齐 importer.rs 的 CAP */
const CAP = 200;

export function isVaultVideo(p) {
  const ext = path.extname(String(p || '')).slice(1).toLowerCase();
  return VAULT_VIDEO_EXT.has(ext);
}

/** 对齐 importer.rs 的 parse_start：'200-399' -> 200，不合规返回 null */
export function parseStart(name) {
  const s = String(name || '');
  const i = s.indexOf('-');
  if (i < 0) return null;
  const a = s.slice(0, i).trim();
  if (!/^\d+$/.test(a)) return null;
  const n = Number(a);
  return Number.isSafeInteger(n) ? n : null;
}

/** 对齐 importer.rs 的 count_videos：只看目录下一层的视频文件 */
export function countVideos(dir) {
  let n = 0;
  let entries = [];
  try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return 0; }
  for (const e of entries) {
    if (!e.isFile()) continue;
    if (isVaultVideo(e.name)) n++;
  }
  return n;
}

/** 列出库根下的区间夹，按起点升序（对齐 plan_targets 的 order） */
export function listBuckets(root) {
  let entries = [];
  try { entries = fs.readdirSync(root, { withFileTypes: true }); } catch { return []; }
  const buckets = [];
  for (const e of entries) {
    if (!e.isDirectory()) continue;
    const start = parseStart(e.name);
    if (start === null) continue; // 非 N-M 命名的子目录不作为落点
    buckets.push({ name: e.name, start, dir: path.join(root, e.name), count: countVideos(path.join(root, e.name)) });
  }
  buckets.sort((a, b) => a.start - b.start);
  return buckets;
}

/** 对齐 importer.rs 的 pick_target：升序第一个未满夹；全满则开新区间 */
export function pickTarget(buckets, allocated) {
  for (const b of buckets) {
    const used = b.count + (allocated.get(b.name) || 0);
    if (used < CAP) return b.name;
  }
  const last = buckets.length ? buckets[buckets.length - 1].start : -CAP;
  const start = last + CAP;
  return `${start}-${start + CAP - 1}`;
}

/** 对齐 importer.rs 的 unique_target_name：原名 → "stem (2).ext" → … */
export function uniqueTargetName(dir, sourceName) {
  const name = sourceName;
  const ext = path.extname(name);
  const stem = path.basename(name, ext);
  if (!fs.existsSync(path.join(dir, name))) return name;
  for (let n = 2; n <= 9999; n++) {
    const candidate = `${stem} (${n})${ext}`;
    if (!fs.existsSync(path.join(dir, candidate))) return candidate;
  }
  const rand = Math.random().toString(16).slice(2, 10);
  return `${stem}-${rand}${ext}`;
}

/** 兜底：影匣没有同名夹时用的备用名（正常走不到） */
function locateVaultctl(cfg) {
  const candidates = [];
  if (cfg.vaultctl) candidates.push(cfg.vaultctl);
  // 影匣安装版永远跟库版本齐平（NSIS 一起装的），优先于源码构建
  candidates.push(path.join(process.env.LOCALAPPDATA || '', '影匣', 'vaultctl.exe'));
  candidates.push(
    'E:/VideoVault/src-tauri/target/release/vaultctl.exe',
    'D:/VideoVault/src-tauri/target/release/vaultctl.exe',
    'C:/VideoVault/src-tauri/target/release/vaultctl.exe',
  );
  for (const c of candidates) {
    if (c && fs.existsSync(c)) return c;
  }
  return null;
}

/** 读影匣桌面端的设置，拿到真正的库根 / 导入根 */
export function readVaultSettings() {
  const appData = process.env.APPDATA || '';
  const p = path.join(appData, 'com.videovault.desktop', 'settings.json');
  try {
    const j = JSON.parse(fs.readFileSync(p, 'utf8'));
    return { library_root: j.library_root || '', import_root: j.import_root || '', settingsPath: p };
  } catch {
    return { library_root: '', import_root: '', settingsPath: p };
  }
}

/** 影匣数据目录：配置 > 桌面端设置 > 空 */
export function resolveVaultRoot(cfg) {
  const s = readVaultSettings();
  return String(cfg.vaultLibrary || s.library_root || s.import_root || '').replace(/\\/g, '/').replace(/\/+$/, '');
}

/**
 * 把一批视频复制进影匣区间夹。
 * 返回 { bucket, tasks:[{source,target}], root }
 */
export function planIntoVault(root, sources) {
  fs.mkdirSync(root, { recursive: true });
  let buckets = listBuckets(root);
  const allocated = new Map();
  const tasks = [];

  for (const src of sources) {
    if (!fs.existsSync(src)) continue;
    if (!isVaultVideo(src)) continue;
    const bucket = pickTarget(buckets, allocated);
    let b = buckets.find((x) => x.name === bucket);
    if (!b) {
      const dir = path.join(root, bucket);
      fs.mkdirSync(dir, { recursive: true });
      b = { name: bucket, start: parseStart(bucket) ?? 0, dir, count: 0 };
      buckets.push(b);
      buckets.sort((x, y) => x.start - y.start);
    }
    allocated.set(b.name, (allocated.get(b.name) || 0) + 1);
    const name = uniqueTargetName(b.dir, path.basename(src));
    tasks.push({ source: src, target: path.join(b.dir, name), bucket: b.name });
  }
  return { root, tasks };
}

/** 复制（异步 + 进度回调：copyFileSync 会阻塞事件循环，/status 就没法实时上报了） */
export async function copyIntoVault(tasks, onProgress) {
  const done = [];
  let idx = 0;
  for (const t of tasks) {
    if (onProgress) onProgress(idx, tasks.length, t);
    try {
      await fs.promises.copyFile(t.source, t.target);
      const size = (await fs.promises.stat(t.target)).size;
      done.push({ ...t, size, ok: true });
    } catch (e) {
      done.push({ ...t, ok: false, error: e.message });
    }
    idx += 1;
    if (onProgress) onProgress(idx, tasks.length, t);
  }
  return done;
}

/**
 * 登记入库：vaultctl import --paths
 * 注意只能用 import，不能对子目录跑 scan --root —— scan 会做全局 mark_missing，
 * 把库内其它视频全标成失效。
 */
export function importIntoVault(cfg, files) {
  const root = resolveVaultRoot(cfg);
  const exe = locateVaultctl(cfg);
  if (!exe) return { ok: false, error: '找不到 vaultctl.exe（影匣的 CLI 未编译？）' };
  if (!files.length) return { ok: true, added: 0, errors: [], skipped: true };

  const errors = [];
  let added = 0;
  const BATCH = 40;
  for (let i = 0; i < files.length; i += BATCH) {
    const batch = files.slice(i, i + BATCH);
    const r = spawnSync(exe, ['--library', root, 'import', '--paths', ...batch],
      { encoding: 'utf8', windowsHide: true, timeout: 180000 });
    if (r.status !== 0) {
      errors.push((r.stderr || r.stdout || `退出码 ${r.status}`).toString().trim().slice(0, 400));
      continue;
    }
    try {
      const j = JSON.parse(r.stdout || '{}');
      added += Number(j.added || 0);
      if (Array.isArray(j.errors)) errors.push(...j.errors);
    } catch {
      // 输出不是 JSON 也算成功（文件已复制，登记与否另行处理）
    }
  }
  return { ok: errors.length === 0, added, errors };
}

/**
 * 后台同步目录集合（collection）。
 * 用 sync-collections 而不是 scan --root：导入新文件已经用 import --paths 登记过了，
 * 这里只需按库内已有路径重建文件夹集合（纯 SQL，不扫盘），
 * 否则每导出一批都要把全库 1919 个文件挨个 stat 一遍，白伤盘。
 */
export function syncVaultInBackground(cfg) {
  const root = resolveVaultRoot(cfg);
  const exe = locateVaultctl(cfg);
  if (!exe || !root) return null;
  const out = path.join(process.env.LOCALAPPDATA || '', '影匣', 'we-export', 'logs', 'vault-scan.log');
  try {
    fs.mkdirSync(path.dirname(out), { recursive: true });
    const fd = fs.openSync(out, 'a');
    const child = spawn(exe, ['--library', root, 'sync-collections'],
      { detached: true, stdio: ['ignore', fd, fd], windowsHide: true });
    child.unref();
    try { fs.closeSync(fd); } catch { /* ignore */ }
    return { pid: child.pid, root };
  } catch {
    return null;
  }
}
