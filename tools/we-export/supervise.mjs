#!/usr/bin/env node
/**
 * 壁纸导出助手 —— 守护进程
 *
 * 为什么要它：之前助手是靠「开机自启」拉起的一次性进程，
 * node 路径一失效、或者进程自己崩了，就永久没人管，
 * 结果就是 WE 里点导出报「连不上导出助手（127.0.0.1:51789）」。
 *
 * 这里做三件事：
 *   1. 拉起 helper.mjs，盯住它，退出就重拉
 *   2. 每 15 秒探一次 /ping，助手卡死（端口在但没响应）也重启
 *   3. 自己写 PID 文件 + 日志，事后能查「到底有没有自启、什么时候崩的」
 *
 * 范式参照 D:\DSH\wb-shim\supervise.mjs（那套在本机验证过可用）。
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn } from 'node:child_process';

const ROOT = import.meta.dirname;
const LOCAL = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
const DATA = path.join(LOCAL, '影匣', 'we-export');
const LOG_DIR = path.join(DATA, 'logs');
const PID_FILE = path.join(DATA, 'supervisor.pid');
const PORT = 51789;
const HEALTH = `http://127.0.0.1:${PORT}/ping`;
const TARGET = path.join(ROOT, 'helper.mjs');
const CHECK_INTERVAL = 15_000;

let child = null;
let stopping = false;
let lastBusyWarnAt = 0;

function ensureDir(p) { fs.mkdirSync(p, { recursive: true }); }

function log(msg) {
  const line = `[${new Date().toISOString()}] ${msg}`;
  try {
    ensureDir(LOG_DIR);
    fs.appendFileSync(path.join(LOG_DIR, `supervisor-${new Date().toISOString().slice(0, 10)}.log`), line + '\n', 'utf8');
  } catch { /* ignore */ }
  try { process.stdout.write(line + '\n'); } catch { /* ignore */ }
}

/** 助手是否活着（端口通且返回 200） */
async function alive() {
  try {
    const r = await fetch(HEALTH, { signal: AbortSignal.timeout(1500) });
    return r.ok;
  } catch {
    return false;
  }
}

function launch() {
  if (child || stopping) return;
  if (!fs.existsSync(TARGET)) { log(`找不到 ${TARGET}，无法拉起助手`); return; }
  let out = null;
  try {
    ensureDir(LOG_DIR);
    out = fs.openSync(path.join(LOG_DIR, `helper-${new Date().toISOString().slice(0, 10)}.log`), 'a');
  } catch { out = 'ignore'; }
  try {
    child = spawn(process.execPath, [TARGET], {
      cwd: ROOT, stdio: ['ignore', out, out], windowsHide: true,
    });
  } catch (e) {
    log(`拉起助手失败: ${e.message}`);
    child = null;
    return;
  }
  const pid = child.pid;
  log(`拉起助手 pid=${pid}`);
  child.on('exit', (code, signal) => {
    child = null;
    if (stopping) { log('守护正在退出，不再重拉'); return; }
    // 退出码 0 + 端口还活着 = 已经有别的实例在跑（helper 遇 EADDRINUSE 会 exit 0）
    if (code === 0) {
      alive().then((ok) => {
        if (ok) { log('端口已有实例占用，本守护退出'); process.exit(0); }
        setTimeout(() => { if (!stopping) launch(); }, 2000);
      });
      return;
    }
    log(`助手退出 code=${code} signal=${signal}，2 秒后重拉`);
    setTimeout(() => { if (!stopping) launch(); }, 2000);
  });
}

/** 已有的守护进程是否还活着 */
function otherSupervisorAlive() {
  try {
    const pid = parseInt(fs.readFileSync(PID_FILE, 'utf8').trim(), 10);
    if (!pid || pid === process.pid) return false;
    process.kill(pid, 0);       // 能发信号 = 进程还在
    return true;
  } catch {
    return false;
  }
}

function writePid() {
  try { ensureDir(DATA); fs.writeFileSync(PID_FILE, String(process.pid), 'utf8'); } catch { /* ignore */ }
}

function clearPid() {
  try {
    const cur = parseInt(fs.readFileSync(PID_FILE, 'utf8').trim(), 10);
    if (cur === process.pid) fs.unlinkSync(PID_FILE);
  } catch { /* ignore */ }
}

async function main() {
  ensureDir(LOG_DIR);
  writePid();
  log(`守护启动 pid=${process.pid} root=${ROOT}`);

  if (await alive()) {
    log('检测到助手已在运行，进入监控模式');
  } else {
    launch();
  }

  setInterval(async () => {
    if (stopping) return;
    // 2026-11-26 事故教训：大导出（枚举整个创意工坊）会把事件循环阻塞几十秒，
    // ping 超时 ≠ 死了。判断「死」的唯一标准是**进程没了**；
    // 进程活着但 ping 不通 = 正在忙，杀了它用户的导出就断了。
    let processAlive = false;
    if (child) {
      try { process.kill(child.pid, 0); processAlive = true; } catch { processAlive = false; }
    }
    const ok = processAlive ? false : await alive();
    if (ok) return;
    if (processAlive) {
      // 只记日志不动手；每条日志都会写盘，但别刷屏 —— 30 秒最多一条
      const now = Date.now();
      if (now - lastBusyWarnAt > 30_000) {
        lastBusyWarnAt = now;
        log(`助手进程活着但暂未响应（多半在处理大导出），不动它 pid=${child.pid}`);
      }
      return;
    }
    if (child) {
      log('助手进程已死，清理后重拉');
      child = null;
    }
    launch();
  }, CHECK_INTERVAL);

  const bye = () => {
    if (stopping) return;
    stopping = true;
    clearPid();
    log('守护退出');
    if (child) { try { child.kill(); } catch { /* ignore */ } }
    process.exit(0);
  };
  process.on('SIGINT', bye);
  process.on('SIGTERM', bye);
  process.on('exit', clearPid);
  process.on('uncaughtException', (e) => log('守护异常: ' + (e && e.stack ? e.stack : e)));
  process.on('unhandledRejection', (e) => log('守护未处理拒绝: ' + (e && e.stack ? e.stack : e)));
}

main();
