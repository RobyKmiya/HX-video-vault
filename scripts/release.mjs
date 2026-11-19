#!/usr/bin/env node
/**
 * 影匣独立发布脚本：抬版本 → 同步三处 → 检查 → 打包 → 安装到本机。
 *
 * 用法：
 *   node scripts/release.mjs             # 补丁位 +1（0.1.0 → 0.1.1）
 *   node scripts/release.mjs --minor
 *   node scripts/release.mjs --set 0.2.0
 *   node scripts/release.mjs --dry-run
 *   node scripts/release.mjs --no-install   # 只打包不安装
 *   node scripts/release.mjs --no-test      # 跳过 Rust 单测（只改了前端时用）
 *
 * 与画匣 release.mjs 完全隔离：
 *   - 只操作本仓库 E:\VideoVault 与安装目录 %LOCALAPPDATA%\影匣；
 *   - 不读取、不修改画匣库（F:\Hthings\BENZI）与画匣进程 archive-gallery.exe；
 *   - 影匣的 exe 名是 video-vault.exe，进程名与画匣不同。
 *
 * 前置：打包前请手动关闭正在运行的影匣（Windows 会锁住 exe 导致链接失败）。
 * 本脚本刻意不调用 tasklist/taskkill —— 那是画匣脚本的做法，且在当前
 * 安全策略下会被拦截；作为一个新产品，影匣不需要那套自动杀进程逻辑。
 */
import { execFileSync, execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

const ROOT = path.resolve(import.meta.dirname, '..');
const TAURI_DIR = path.join(ROOT, 'src-tauri');
const PKG = path.join(ROOT, 'package.json');
const CARGO = path.join(TAURI_DIR, 'Cargo.toml');
const CONF = path.join(TAURI_DIR, 'tauri.conf.json');
const INSTALL_DIR = path.join(os.homedir(), 'AppData', 'Local', '影匣');
const INSTALLED_EXE = path.join(INSTALL_DIR, 'video-vault.exe');
const PRODUCT = '影匣';

const argv = process.argv.slice(2);
const dryRun = argv.includes('--dry-run');
const doInstall = !argv.includes('--no-install');
const skipTests = argv.includes('--no-test');
const bumpKind = argv.includes('--minor') ? 'minor' : argv.includes('--major') ? 'major' : 'patch';
const setIndex = argv.indexOf('--set');
const explicitVersion = setIndex >= 0 ? argv[setIndex + 1] : null;

const log = (m) => console.log(`\n▸ ${m}`);
const ok = (m) => console.log(`  ✓ ${m}`);
const warn = (m) => console.log(`  ! ${m}`);

function step(title, fn) {
  log(title);
  if (dryRun) { console.log('  (dry-run，跳过执行)'); return; }
  fn();
}

function readVersion() {
  return JSON.parse(fs.readFileSync(PKG, 'utf8')).version;
}

function bump(version, kind) {
  const [a, b, c] = version.split('.').map(Number);
  if (kind === 'major') return `${a + 1}.0.0`;
  if (kind === 'minor') return `${a}.${b + 1}.0`;
  return `${a}.${b}.${c + 1}`;
}

/** 三处版本号必须一致，否则 Tauri 按 conf 打包、Cargo 里却是另一个。 */
function syncVersion(next) {
  const pkg = JSON.parse(fs.readFileSync(PKG, 'utf8'));
  pkg.version = next;
  fs.writeFileSync(PKG, `${JSON.stringify(pkg, null, 2)}\n`, 'utf8');

  const cargo = fs.readFileSync(CARGO, 'utf8');
  const cargoNext = cargo.replace(/^(version\s*=\s*)"[^"]+"/m, `$1"${next}"`);
  // 同版本重发时 replace 不会改变内容，这里要能识别「已经是目标版本」而不是报错
  const already = new RegExp(`^version\\s*=\\s*"${next.replace(/\./g, '\\.')}"`, 'm').test(cargo);
  if (cargoNext === cargo && !already) throw new Error('Cargo.toml 里没找到 version 字段');
  fs.writeFileSync(CARGO, cargoNext, 'utf8');

  const conf = JSON.parse(fs.readFileSync(CONF, 'utf8'));
  conf.version = next;
  fs.writeFileSync(CONF, `${JSON.stringify(conf, null, 2)}\n`, 'utf8');
}

function run(cmd, args, cwd = ROOT) {
  execFileSync(cmd, args, { cwd, stdio: 'inherit', shell: false });
}

function runNpm(script) {
  // Windows 上 npm 是 .cmd，Node 直接 spawnSync 会 EINVAL，必须走 shell
  if (process.platform === 'win32') {
    execSync(`npm run ${script}`, { cwd: ROOT, stdio: 'inherit' });
  } else {
    run('npm', ['run', script]);
  }
}

function warnIfDirty() {
  try {
    const s = execSync('git status --porcelain', { cwd: ROOT, encoding: 'utf8' }).trim();
    if (s) {
      warn('工作区有未提交改动，它们会被打包进这个版本：');
      for (const line of s.split('\n').slice(0, 10)) console.log(`    ${line}`);
    }
  } catch { /* 不在 git 仓库则跳过 */ }
}

// ── 主流程 ────────────────────────────────────────────────
const current = readVersion();
const next = explicitVersion ?? bump(current, bumpKind);
if (explicitVersion && !/^\d+\.\d+\.\d+$/.test(explicitVersion)) {
  console.error(`版本号格式应为 x.y.z，收到：${explicitVersion}`);
  process.exit(1);
}

console.log(`${PRODUCT} · 独立发布`);
console.log(`  当前版本 ${current} → ${next}${dryRun ? '（dry-run）' : ''}`);
console.log(`  安装目录 ${INSTALL_DIR}`);
console.log(`  注意：只操作 ${PRODUCT}，不碰画匣及其库`);
warnIfDirty();

step(`抬版本并同步三处（package.json / Cargo.toml / tauri.conf.json）`, () => {
  syncVersion(next);
  ok(`三处已同步为 ${next}`);
});

step('前端类型检查', () => { runNpm('check'); ok('tsc 通过'); });

skipTests ? warn('已跳过 Rust 单测（--no-test）') : step('Rust 单元测试（离线）', () => {
  execSync('cargo test --offline --manifest-path src-tauri/Cargo.toml --all-targets',
    { cwd: ROOT, stdio: 'inherit', env: { ...process.env, CARGO_NET_OFFLINE: 'true' } });
  ok('测试通过');
});

step('打包（含 Rust release 编译 + NSIS）', () => {
  execSync('npm run tauri build',
    { cwd: ROOT, stdio: 'inherit', env: { ...process.env, CARGO_NET_OFFLINE: 'true' } });
  ok('安装包已生成');
});

const setupPath = path.join(TAURI_DIR, 'target', 'release', 'bundle', 'nsis', `${PRODUCT}_${next}_x64-setup.exe`);

step('核对安装包', () => {
  if (!fs.existsSync(setupPath)) throw new Error(`找不到安装包：${setupPath}`);
  ok(`安装包：${setupPath}（${(fs.statSync(setupPath).size / 1048576).toFixed(2)} MiB）`);
});

if (doInstall) {
  step(`安装到 ${INSTALL_DIR}`, () => {
    execSync(`"${setupPath}" /S`, { stdio: 'inherit' });
    ok('安装完成');
  });
  step('验证', () => {
    if (!fs.existsSync(INSTALLED_EXE)) throw new Error(`安装后找不到 exe：${INSTALLED_EXE}`);
    const stat = fs.statSync(INSTALLED_EXE);
    ok(`exe 时间戳 ${stat.mtime.toLocaleString()}`);
    if (readVersion() !== next) throw new Error('版本号校验不一致');
    ok(`版本号一致：${next}`);
    try {
      const fd = fs.openSync(INSTALLED_EXE, 'r+');
      fs.closeSync(fd);
      ok('exe 未被锁定');
    } catch {
      warn('exe 仍被占用，可能影匣还在运行');
    }
  });
} else {
  warn('已跳过安装（--no-install）');
}

if (dryRun) {
  console.log('\n(dry-run 结束，未改动任何文件)');
} else {
  console.log(`\n完成。新版 ${next} 已${doInstall ? `安装，可到 ${INSTALL_DIR} 打开` : '打包'}。`);
  console.log(`安装包留档：${setupPath}`);
}
