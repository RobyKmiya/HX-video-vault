#!/usr/bin/env node
/**
 * 安装器（影匣内置组件）：给 Wallpaper Engine 的 UI 打注入补丁 + 部署本地导出助手
 *
 *   node install.mjs              安装 / 重打补丁（Steam 更新后重跑一次）
 *   node install.mjs --status     只看状态
 *   node install.mjs --uninstall  还原 UI、移除自启
 *
 * 三件事必须一起做，缺一个导出就会报「连不上导出助手」：
 *   1. 给 WE 的 ui/dist/index.html 注入 scripts/_wx/export.js
 *   2. 把助手部署到稳定目录 %LOCALAPPDATA%\影匣\we-export\app
 *      —— 复合到影匣（2026-11-27）：跟影匣本体同一个安装目录，
 *         卸载影匣时 NSIS 连带清掉助手；vaultctl 也直接用影匣安装版（版本永远跟库齐平）
 *   3. 装开机自启三重保险：注册表 HKCU\...\Run（主）+ Startup 文件夹 + 计划任务
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = import.meta.dirname;
const LOCAL = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
/** 影匣的安装根目录（NSIS 装在这，卸载时整目录删除，助手跟着一起走） */
const VV_HOME = path.join(LOCAL, '影匣');
const DATA = path.join(VV_HOME, 'we-export');
const RUNTIME = path.join(DATA, 'app');
const LEGACY_DATA = path.join(LOCAL, 'WeExportHelper');   // 2026-11-27 之前的旧位置，迁移时清掉
const APPDATA = process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming');
const STARTUP = path.join(APPDATA, 'Microsoft', 'Windows', 'Start Menu', 'Programs', 'Startup');
const STARTUP_VBS = path.join(STARTUP, '壁纸导出助手.vbs');
const RUNTIME_VBS = path.join(RUNTIME, '启动助手.vbs');
const RUN_KEY = 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run';
const RUN_NAME = 'WallpaperExportHelper';
const TASK_NAME = 'WallpaperExportHelper';
const TAG = '<!--wx-export-->';
const SCRIPT_TAG = '<script src="scripts/_wx/export.js"></script>' + TAG;

/** vaultctl 探测顺序：影匣安装版（永远跟库版本齐平）→ 源码构建 → 用户显式配置 */
const VAULTCTL_CANDIDATES = [
  path.join(VV_HOME, 'vaultctl.exe'),
  'E:/VideoVault/src-tauri/target/release/vaultctl.exe',
];

const WE_CANDIDATES = [
  'D:/Steam/steamapps/common/wallpaper_engine',
  'C:/Program Files (x86)/Steam/steamapps/common/wallpaper_engine',
  'C:/Program Files/Steam/steamapps/common/wallpaper_engine',
  'E:/Steam/steamapps/common/wallpaper_engine',
  'F:/Steam/steamapps/common/wallpaper_engine',
];

// 部署到运行时目录的文件（相对 ROOT）。
// 运行时目录必须自包含：会话目录会被回收，届时还要能重装（Steam 更新后）。
const RUNTIME_FILES = [
  'helper.mjs', 'vault.mjs', 'supervise.mjs', 'config.json', '启动助手.vbs', 'install.mjs',
  'inject/export.js', 'README.md',
  'bin/notify.ps1', 'bin/start-helper.vbs',
];

const args = process.argv.slice(2);
const MODE = args.includes('--uninstall') ? 'uninstall' : args.includes('--status') ? 'status' : 'install';
const log = (s) => process.stdout.write(s + '\n');

/* ------------------------------- 小工具 ------------------------------- */

function readJSON(p) {
  try { return JSON.parse(fs.readFileSync(p, 'utf8')); } catch { return {}; }
}

function writeJSON(p, obj) {
  fs.mkdirSync(path.dirname(p), { recursive: true });
  const body = JSON.stringify(obj, null, 2) + '\n';
  try {
    fs.writeFileSync(p, body, 'utf8');
  } catch (e) {
    if (e.code !== 'EPERM') throw e;
    // E 盘的文件守卫拦「改写已存在文件」：写临时文件后原子替换绕开（BOARD 坑 9 同款）
    const tmp = p + '.tmp-' + process.pid;
    fs.writeFileSync(tmp, body, 'utf8');
    fs.renameSync(tmp, p);
  }
}

/** 助手是否在跑 */
function helperAlive() {
  const r = spawnSync(process.execPath, ['-e', `
    const http=require('http');
    http.get({host:'127.0.0.1',port:51789,path:'/ping',timeout:1200},res=>console.log('ok')).on('error',()=>console.log('down'));
  `], { encoding: 'utf8', timeout: 4000 });
  return (r.stdout || '').includes('ok');
}

/** 找 node.exe：系统级优先，WorkBuddy 自带的兜底 */
function findNode() {
  const cands = [
    'C:/Program Files/nodejs/node.exe',
    'C:/Program Files (x86)/nodejs/node.exe',
    path.join(process.env.LOCALAPPDATA || '', 'Programs', 'nodejs', 'node.exe'),
    process.execPath,
  ];
  for (const c of cands) {
    if (c && fs.existsSync(c)) return c.replace(/\\/g, '\\');
  }
  return process.execPath;
}

/** 读注册表自启项 */
function readRunEntry() {
  let r = spawnSync('reg.exe', ['query', RUN_KEY, '/v', RUN_NAME], { encoding: 'utf8', windowsHide: true, timeout: 8000 });
  if (r.status === 0) {
    const m = /REG_SZ\s+(.+)\r?\n?$/m.exec(r.stdout || '');
    if (m) return m[1].trim();
  }
  // reg.exe 不可用时走 PowerShell（.NET 注册表提供程序）
  const ps = `$v=(Get-ItemProperty -Path '${RUN_KEY.replace(/\\/g, '\\\\')}' -Name '${RUN_NAME}' -ErrorAction SilentlyContinue).${RUN_NAME}; if($v){$v}`;
  r = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', ps],
    { encoding: 'utf8', windowsHide: true, timeout: 20000 });
  if (r.status === 0) {
    const v = (r.stdout || '').trim();
    if (v) return v;
  }
  return null;
}

/** 写注册表自启项 */
function writeRunEntry(command) {
  let r = spawnSync('reg.exe', ['add', RUN_KEY, '/v', RUN_NAME, '/t', 'REG_SZ', '/d', command, '/f'],
    { encoding: 'utf8', windowsHide: true, timeout: 8000 });
  if (r.status === 0) return true;
  // reg.exe 被禁用 / 不可用时走 PowerShell
  const esc = command.replace(/'/g, "''");
  const ps = `$p='${RUN_KEY.replace(/\\/g, '\\\\')}'; New-ItemProperty -Path $p -Name '${RUN_NAME}' -Value '${esc}' -PropertyType String -Force | Out-Null`;
  r = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', ps],
    { encoding: 'utf8', windowsHide: true, timeout: 20000 });
  return r.status === 0;
}

function deleteRunEntry() {
  let r = spawnSync('reg.exe', ['delete', RUN_KEY, '/v', RUN_NAME, '/f'], { encoding: 'utf8', windowsHide: true, timeout: 8000 });
  if (r.status === 0) return true;
  const ps = `Remove-ItemProperty -Path '${RUN_KEY.replace(/\\/g, '\\\\')}' -Name '${RUN_NAME}' -ErrorAction SilentlyContinue`;
  r = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', ps],
    { encoding: 'utf8', windowsHide: true, timeout: 20000 });
  return r.status === 0;
}

/* ------------------------------ WE 定位 ------------------------------- */

function detectWE() {
  const cfg = readJSON(path.join(ROOT, 'config.json'));
  if (cfg.library) {
    const guess = path.resolve(cfg.library, '..', '..').replace(/\\/g, '/');
    if (fs.existsSync(path.join(guess, 'ui', 'dist', 'index.html'))) return guess;
  }
  for (const c of WE_CANDIDATES) {
    if (fs.existsSync(path.join(c, 'ui', 'dist', 'index.html'))) return c;
  }
  return null;
}

/* ------------------------------ 状态输出 ------------------------------ */

function status(weDir) {
  const uiFile = weDir ? path.join(weDir, 'ui', 'dist', 'index.html') : null;
  const patched = uiFile && fs.existsSync(uiFile)
    ? fs.readFileSync(uiFile, 'utf8').includes('scripts/_wx/export.js')
    : false;
  const backup = uiFile ? fs.existsSync(uiFile + '.wx-backup') : false;
  const alive = helperAlive();
  const runEntry = readRunEntry();
  const lastAutostart = (() => {
    try { return fs.readFileSync(path.join(DATA, 'autostart.txt'), 'utf8').trim(); } catch { return ''; }
  })();
  const supervisorPid = (() => {
    try { return fs.readFileSync(path.join(DATA, 'supervisor.pid'), 'utf8').trim(); } catch { return ''; }
  })();
  const cfg = readJSON(path.join(RUNTIME, 'config.json'));
  const vaultRoot = cfg.vaultLibrary || '';

  log('=== 状态 ===');
  log(`WE 目录        : ${weDir || '未找到'}`);
  log(`UI 补丁        : ${patched ? '已打' : '未打'}`);
  log(`原文件备份     : ${backup ? '有 (index.html.wx-backup)' : '无'}`);
  log(`助手服务       : ${alive ? '运行中 (127.0.0.1:51789)' : '未运行'}`);
  log(`守护进程       : ${supervisorPid ? `pid ${supervisorPid}` : '未运行'}`);
  log(`运行时目录     : ${RUNTIME} ${fs.existsSync(RUNTIME) ? '' : '(不存在)'}`);
  log(`注册表自启     : ${runEntry ? '已设置 → ' + runEntry : (lastAutostart ? `上次安装写入的是 → ${lastAutostart}（当前环境查不到注册表）` : '未设置')}`);
  log(`启动文件夹兜底 : ${fs.existsSync(STARTUP_VBS) ? '已设置' : '未设置'}`);
  log(`影匣落点       : ${vaultRoot || '(未配置，读桌面端 settings.json)'}`);
  return { patched, backup, alive, runEntry };
}

/* ------------------------------ UI 补丁 ------------------------------ */

function patchUI(weDir) {
  const uiDist = path.join(weDir, 'ui', 'dist');
  const indexPath = path.join(uiDist, 'index.html');
  const backupPath = indexPath + '.wx-backup';

  let html = fs.readFileSync(indexPath, 'utf8');

  // 已经有补丁就不再插
  if (html.includes('scripts/_wx/export.js')) {
    log('UI 补丁已存在，跳过插入');
  } else {
    // 只在第一次备份（避免把打了补丁的版本当成原版备份）
    if (!fs.existsSync(backupPath)) {
      fs.copyFileSync(indexPath, backupPath);
      log(`已备份原文件 -> ${path.basename(backupPath)}`);
    } else {
      log('备份已存在，保留最早的原始版本');
    }
    if (!html.includes('</head>')) throw new Error('index.html 结构异常，找不到 </head>');
    html = html.replace('</head>', SCRIPT_TAG + '</head>');
    fs.writeFileSync(indexPath, html, 'utf8');
    log('已写入 UI 补丁');
  }

  // 部署注入脚本
  const destDir = path.join(uiDist, 'scripts', '_wx');
  fs.mkdirSync(destDir, { recursive: true });
  const src = path.join(ROOT, 'inject', 'export.js');
  const dest = path.join(destDir, 'export.js');
  fs.copyFileSync(src, dest);
  log(`已部署注入脚本 -> ${dest} (${fs.statSync(dest).size} bytes)`);

  // 校验真的写进去了（这个盘有时会静默丢弃写入）
  const check = fs.readFileSync(indexPath, 'utf8');
  if (!check.includes('scripts/_wx/export.js')) throw new Error('补丁校验失败：index.html 里没找到脚本引用');
  if (!fs.existsSync(dest)) throw new Error('补丁校验失败：注入脚本不存在');
  return { indexPath, dest };
}

/* --------------------------- 运行时目录部署 --------------------------- */

function syncConfig(weDir) {
  // 把配置里的 WE 路径对齐到实际安装位置，并补上影匣落点
  const cfgPath = path.join(ROOT, 'config.json');
  const cfg = readJSON(cfgPath);
  if (weDir) {
    const we = weDir.replace(/\\/g, '/');
    cfg.weRoot = we;
    cfg.projectsRoot = `${we}/projects`;
    cfg.library = `${we}/projects/myprojects`;
  }
  if (!cfg.vaultLibrary) {
    // 读影匣桌面端的设置，拿到真正的库根
    const s = readJSON(path.join(APPDATA, 'com.videovault.desktop', 'settings.json'));
    cfg.vaultLibrary = (s.library_root || s.import_root || '').replace(/\\/g, '/');
  }
  writeJSON(cfgPath, cfg);
  return cfg;
}

function deployRuntime(cfg) {
  fs.mkdirSync(RUNTIME, { recursive: true });
  for (const rel of RUNTIME_FILES) {
    const src = path.join(ROOT, rel);
    const dest = path.join(RUNTIME, rel);
    if (!fs.existsSync(src)) { log(`跳过（源文件不存在）: ${rel}`); continue; }
    // 从运行时目录里再装一次时，源和目标是同一个文件，跳过
    if (path.resolve(src) === path.resolve(dest)) continue;
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.copyFileSync(src, dest);
  }
  // 配置也同步一份（运行时目录里的 helper.mjs 读的是自己旁边的 config.json）
  writeJSON(path.join(RUNTIME, 'config.json'), cfg);

  fs.writeFileSync(path.join(DATA, 'root.txt'), RUNTIME, 'utf8');
  fs.writeFileSync(path.join(DATA, 'node-path.txt'), process.execPath, 'utf8');
  log(`运行时目录 -> ${RUNTIME}`);
  return RUNTIME;
}

/* ------------------------------ 自启设置 ------------------------------ */

function setupAutostart() {
  // 主：注册表 HKCU Run（用户级，无需管理员；本机实测可靠）
  const nodeExe = findNode();
  const supervisor = path.join(RUNTIME, 'supervise.mjs');
  const cmd = `"${nodeExe}" "${supervisor}"`;

  // 落地一份"上次装了什么"，万一注册表查询不可用（受限环境），状态页也不会误报成"未设置"
  try {
    fs.writeFileSync(path.join(DATA, 'autostart.txt'), cmd + '\n', 'utf8');
  } catch { /* ignore */ }

  if (writeRunEntry(cmd)) {
    log(`注册表自启 -> ${RUN_KEY}\\${RUN_NAME} = ${cmd}`);
  } else {
    log('注册表自启写入失败（稍后可用 Startup 文件夹兜底）');
  }

  // 兜底一：Startup 文件夹里的 VBS（自带 node 多路径探测）
  try {
    fs.mkdirSync(STARTUP, { recursive: true });
    fs.copyFileSync(path.join(RUNTIME, '启动助手.vbs'), STARTUP_VBS);
    log(`启动文件夹兜底 -> ${STARTUP_VBS}`);
  } catch (e) {
    log(`启动文件夹兜底失败: ${e.message}`);
  }

  // 兜底二：计划任务（登录时触发）。比 Run 项更抗杀软与启动项清理；
  // 2026-11-26 的事故证明单一自启通道总有一天掉链子，三重保险才睡得着。
  const taskCmd = `"${nodeExe}" "${supervisor}"`;
  const r = spawnSync('schtasks.exe', [
    '/create', '/tn', 'WallpaperExportHelper', '/f',
    '/sc', 'onlogon', '/rl', 'limited', '/tr', taskCmd,
  ], { encoding: 'utf8', windowsHide: true, timeout: 20000 });
  if (r.status === 0) {
    log('计划任务兜底 -> WallpaperExportHelper（登录时触发）');
  } else {
    log(`计划任务兜底失败（不影响其它两条自启）: ${(r.stderr || r.stdout || '').trim().slice(0, 120)}`);
  }
}

function removeAutostart() {
  if (deleteRunEntry()) log('已移除注册表自启');
  if (fs.existsSync(STARTUP_VBS)) { fs.unlinkSync(STARTUP_VBS); log('已移除启动文件夹自启'); }
  spawnSync('schtasks.exe', ['/delete', '/tn', 'WallpaperExportHelper', '/f'],
    { encoding: 'utf8', windowsHide: true, timeout: 15000 });
  log('已移除计划任务自启（若存在）');
}

/* ------------------------------ 启动助手 ------------------------------ */

/** 同步睡一会儿（node 没有 sleep，用 Atomics.wait 凑） */
function sleepSync(ms) {
  try {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
  } catch { /* ignore */ }
}

function startHelper() {
  if (helperAlive()) { log('助手已在运行'); return true; }

  const supervisor = path.join(RUNTIME, 'supervise.mjs');
  if (!fs.existsSync(supervisor)) { log('缺少 supervise.mjs，无法启动'); return false; }

  const child = spawn(process.execPath, [supervisor], {
    cwd: RUNTIME, detached: true, stdio: 'ignore', windowsHide: true,
  });
  child.unref();
  log(`已启动守护进程 (pid ${child.pid})`);

  // 等一下确认真的起来了（detached 子进程在个别环境会被回收）
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    if (helperAlive()) { log('助手已就绪 (127.0.0.1:51789)'); return true; }
    sleepSync(300);
  }
  log(`⚠ 助手没响应，请手动运行：node "${supervisor}"`);
  return false;
}

/* ------------------------------ 卸载 ------------------------------ */

function uninstall(weDir) {
  const uiDist = weDir ? path.join(weDir, 'ui', 'dist') : null;
  if (uiDist) {
    const indexPath = path.join(uiDist, 'index.html');
    const backupPath = indexPath + '.wx-backup';
    if (fs.existsSync(backupPath)) {
      fs.copyFileSync(backupPath, indexPath);
      fs.unlinkSync(backupPath);
      log('已还原 index.html');
    } else if (fs.existsSync(indexPath)) {
      let html = fs.readFileSync(indexPath, 'utf8');
      html = html.replace(SCRIPT_TAG, '');
      fs.writeFileSync(indexPath, html, 'utf8');
      log('已移除补丁（无备份，只删脚本引用）');
    }
    const destDir = path.join(uiDist, 'scripts', '_wx');
    if (fs.existsSync(destDir)) {
      fs.rmSync(destDir, { recursive: true, force: true });
      log('已删除注入脚本目录');
    }
  }
  removeAutostart();
  log('助手进程请手动结束：node supervise.mjs / node helper.mjs');
}

/* ------------------------------ 主流程 ------------------------------ */

/**
 * 一次性迁移：2026-11-27 之前助手住在独立的 %LOCALAPPDATA%\WeExportHelper，
 * 复合到影匣后改住 %LOCALAPPDATA%\影匣\we-export（卸载影匣时连带清理）。
 * 旧守护进程通知退出（它会带走自己的 helper），旧目录改名弃用。
 */
function cleanupLegacy() {
  if (!fs.existsSync(LEGACY_DATA)) return;
  try {
    const oldPid = parseInt(fs.readFileSync(path.join(LEGACY_DATA, 'supervisor.pid'), 'utf8').trim(), 10);
    if (oldPid) process.kill(oldPid, 'SIGTERM');
    log(`已通知旧守护进程（pid ${oldPid}）退出`);
  } catch { /* 旧守护不在了 */ }
  try {
    const retired = LEGACY_DATA + '.retired-' + Date.now();
    fs.renameSync(LEGACY_DATA, retired);
    log(`旧运行时目录已弃用 → ${retired}（可手动删除）`);
  } catch {
    log(`提示：旧目录 ${LEGACY_DATA} 暂被占用，进程退完后可手动删除`);
  }
}

function main() {
  const weDir = detectWE();
  if (MODE === 'status') { status(weDir); return; }
  if (!weDir) throw new Error('没找到 Wallpaper Engine（含 ui/dist/index.html）');
  log(`WE 目录: ${weDir}`);

  if (MODE === 'uninstall') { uninstall(weDir); return; }

  cleanupLegacy();

  log('\n=== 打补丁 ===');
  patchUI(weDir);

  log('\n=== 部署助手 ===');
  const cfg = syncConfig(weDir);
  deployRuntime(cfg);
  // 2026-11-28：助手生命周期改由影匣本体托管（影匣启动时拉起 helper.mjs 并守护，
  // 死了自动重启），不再装开机自启 / 计划任务，避免两套守护互相打架。
  removeAutostart();
  log('助手生命周期已交由影匣托管：影匣运行时自动拉起并守护，面板「壁纸导出」可启停/看日志');

  log('\n=== 完成 ===');
  log(`助手运行时: ${RUNTIME}（随影匣安装目录，卸载影匣时连带清理）`);
  log(`影匣落点: ${cfg.vaultLibrary || '(读桌面端 settings.json)'}`);
  log('补丁已生效：完全退出壁纸引擎（托盘图标右键 → 退出）后重新打开，就能看到注入效果。');
}

try { main(); } catch (e) {
  process.stderr.write('\n失败: ' + (e.stack || e.message) + '\n');
  process.exit(1);
}
