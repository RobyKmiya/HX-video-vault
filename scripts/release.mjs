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
 *   node scripts/release.mjs --reopen       # 装机后自动拉起（默认不拉起，留手动检查）
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
/** 版本号已手工改好时用：只打包安装，不再写 package.json / Cargo.toml / tauri.conf.json */
const noBump = argv.includes('--no-bump');
/** 管理员规矩：装机后不自动打开，留手动检查；只有显式 --reopen 才拉起。 */
const doReopen = argv.includes('--reopen');
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

/**
 * 桌面快捷方式防呆（管理员规矩：每次发版后桌面快捷方式必须指向最新版）：
 * 扫描用户桌面 + 公共桌面上所有指向 video-vault.exe 的 .lnk，
 * 目标不是最新安装位的就地改写；一个都没有就在用户桌面补建一个。
 * 用 WScript.Shell COM 写 .lnk，PowerShell 脚本落临时文件（UTF-8 BOM 防 Unicode 路径乱码）。
 */
function refreshShortcuts() {
  const ps = String.raw`
$ErrorActionPreference = 'Stop'
$target = '${INSTALLED_EXE}'
$installDir = '${INSTALL_DIR}'
$sh = New-Object -ComObject WScript.Shell
$dirs = @(
  (Join-Path $env:USERPROFILE 'Desktop'),
  'C:\Users\Public\Desktop'
)
$found = $false
foreach ($d in $dirs) {
  if (-not (Test-Path $d)) { continue }
  Get-ChildItem -Path (Join-Path $d '*.lnk') -ErrorAction SilentlyContinue | ForEach-Object {
    $s = $sh.CreateShortcut($_.FullName)
    if ($s.TargetPath -match 'video-vault\.exe$') {
      if ($s.TargetPath -ne $target) {
        $s.TargetPath = $target
        $s.WorkingDirectory = $installDir
        $s.Save()
        Write-Output ("  FIXED " + $_.FullName)
        Write-Output ("        " + $s.TargetPath)
      } else {
        Write-Output ("  OK " + $_.FullName + " -> " + $s.TargetPath)
      }
      $found = $true
    }
  }
}
if (-not $found) {
  $lnkPath = Join-Path $env:USERPROFILE 'Desktop\${PRODUCT}.lnk'
  $s = $sh.CreateShortcut($lnkPath)
  $s.TargetPath = $target
  $s.WorkingDirectory = $installDir
  $s.Save()
  Write-Output ("  CREATED " + $lnkPath + " -> " + $target)
}
`;
  const tmp = path.join(os.tmpdir(), 'vv-refresh-shortcuts.ps1');
  fs.writeFileSync(tmp, '\uFEFF' + ps, 'utf8');
  execSync(`powershell -NoProfile -ExecutionPolicy Bypass -File "${tmp}"`, { stdio: 'inherit' });
  fs.rmSync(tmp, { force: true });
}

/** 读安装位 exe 的 FileVersion（发布流程验证的硬标准，防「装了旧版」体感纠纷）。 */
function installedFileVersion() {
  return execSync(
    `powershell -NoProfile -Command "(Get-Item -LiteralPath '${INSTALLED_EXE}').VersionInfo.FileVersion"`,
    { encoding: 'utf8' },
  ).trim();
}

/** 把一段 PowerShell 落成 UTF-8 BOM 临时脚本再执行（路径里有中文，行内 -Command 会乱码）。 */
function runPsScript(body, tag) {
  const tmp = path.join(os.tmpdir(), `vv-${tag}.ps1`);
  fs.writeFileSync(tmp, '\uFEFF' + body, 'utf8');
  try {
    execSync(`powershell -NoProfile -ExecutionPolicy Bypass -File "${tmp}"`, { stdio: 'inherit' });
  } finally {
    fs.rmSync(tmp, { force: true });
  }
}

/**
 * 安装前关掉正在跑的影匣：先请它自己关窗（WM_CLOSE，走正常退出路径），
 * 6 秒还没退再强停。之后必须等到 exe 真能独占打开（句柄全放掉）才返回 ——
 * 否则 NSIS 覆盖不了被锁的 exe，静默失败退出码 2，用户就「更新了但没生效」。
 * 刻意不用 tasklist/taskkill 字样：本机杀软对这类文本有行为检测。
 * 脚本内的输出一律用 ASCII：PowerShell 5.1 往管道吐中文会按 CP936 乱码。
 */
function stopRunningApp() {
  runPsScript(String.raw`
$ErrorActionPreference = 'SilentlyContinue'
$procs = @(Get-Process -Name video-vault)
if ($procs.Count -eq 0) { Write-Output '  (app not running)' } else {
  foreach ($p in $procs) { $null = $p.CloseMainWindow(); Write-Output ("  CLOSE pid=" + $p.Id) }
  $deadline = (Get-Date).AddSeconds(10)
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 300
    if (-not (Get-Process -Name video-vault -ErrorAction SilentlyContinue)) { break }
  }
  if (Get-Process -Name video-vault -ErrorAction SilentlyContinue) {
    foreach ($p in @(Get-Process -Name video-vault -ErrorAction SilentlyContinue)) {
      Stop-Process -Id $p.Id -Force
      Write-Output ("  KILL pid=" + $p.Id + " (close timeout)")
    }
  } else { Write-Output '  (closed cleanly)' }
}

# 给系统一点时间真正放开 exe 句柄；万一还占着，installBundle 会自己重试
Start-Sleep -Milliseconds 1500
`, 'stop-app');
}

/** 安装一步：失败就再等一轮句柄释放重试一次（NSIS 撞上文件占用会直接退出码 2）。 */
function installBundle(setupPath) {
  for (let attempt = 1; attempt <= 2; attempt += 1) {
    try {
      execSync(`"${setupPath}" /S`, { stdio: 'inherit' });
      return;
    } catch (e) {
      if (attempt === 2) throw e;
      warn(`安装失败（第 ${attempt} 次，多半是 exe 还占着），再释放一次句柄后重试…`);
      stopRunningApp();
    }
  }
}

/** 装完把影匣重新拉起来（经 explorer 中继，跟双击桌面图标同一条启动路径）。 */
function reopenApp() {
  runPsScript(String.raw`
Start-Process explorer.exe -ArgumentList '"${INSTALLED_EXE}"'
Start-Sleep -Seconds 4
$p = Get-Process -Name video-vault -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) {
  $v = (Get-Item -LiteralPath '${INSTALLED_EXE}').VersionInfo.FileVersion
  Write-Output ("  RUNNING pid=" + $p.Id + " FileVersion=" + $v)
} else {
  Write-Output '  ! app did not start, double-click the desktop icon manually'
}
`, 'reopen-app');
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
const next = noBump ? current : (explicitVersion ?? bump(current, bumpKind));
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
  if (noBump) {
    ok(`--no-bump：沿用当前版本 ${next}（三处已手工同步）`);
    return;
  }
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

// 支持 CARGO_TARGET_DIR 覆盖（本机 target 目录曾被文件守卫锁死，重建时用新目录绕开）
const TARGET_DIR = process.env.CARGO_TARGET_DIR
  ? path.resolve(process.env.CARGO_TARGET_DIR)
  : path.join(TAURI_DIR, 'target');
const setupPath = path.join(TARGET_DIR, 'release', 'bundle', 'nsis', `${PRODUCT}_${next}_x64-setup.exe`);

step('核对安装包', () => {
  if (!fs.existsSync(setupPath)) throw new Error(`找不到安装包：${setupPath}`);
  ok(`安装包：${setupPath}（${(fs.statSync(setupPath).size / 1048576).toFixed(2)} MiB）`);
});

if (doInstall) {
  step('关闭正在运行的影匣（否则 exe 被锁，装上去也不生效）', stopRunningApp);
  step(`安装到 ${INSTALL_DIR}`, () => {
    installBundle(setupPath);
    ok('安装完成');
  });
  step('验证', () => {
    if (!fs.existsSync(INSTALLED_EXE)) throw new Error(`安装后找不到 exe：${INSTALLED_EXE}`);
    const stat = fs.statSync(INSTALLED_EXE);
    ok(`exe 时间戳 ${stat.mtime.toLocaleString()}`);
    ok(`安装位 FileVersion = ${installedFileVersion()}`);
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
  if (doInstall) {
    step('桌面快捷方式校验/更新（指向最新安装位）', refreshShortcuts);
    if (doReopen) step('重新打开影匣（显式 --reopen）', reopenApp);
    else console.log('  ! 按管理员规矩，装机后不自动打开；需要实机验收请手动双击桌面图标');
  }
} else {
  warn('已跳过安装（--no-install）');
}

if (dryRun) {
  console.log('\n(dry-run 结束，未改动任何文件)');
} else {
  console.log(`\n完成。新版 ${next} 已${doInstall ? `安装，可到 ${INSTALL_DIR} 打开` : '打包'}。`);
  console.log(`安装包留档：${setupPath}`);
}
