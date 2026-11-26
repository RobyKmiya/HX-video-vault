# 影匣源码异地备份
#
# 背景：2026-11-26 E 盘（USB 外置 NVMe）出现过 ERROR_DEVICE_HARDWARE_ERROR(483)
# 设备层断连，E:\VideoVault 整目录读不出来。影匣源码没有远程仓库、没有其他副本，
# 那块盘是唯一存放地。这个脚本把源码 + .git 历史复制到 D 盘，留最近 N 份。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File D:\VideoVault\scripts\backup-source.ps1
#   加 -Quiet 只在失败时输出
#
# 源目录 2026-11-27 03:30 修正为 D:\VideoVault（原先写死 E:\VideoVault，E 盘源码清空后
# 每日 03:00 的自动化会静默失败、备份断流 —— 踩过一次）。
#
# 排除项说明：
#   src-tauri\target  编译产物，31.7GB，千万别备份
#   node_modules      依赖，可重装
#   dist              前端构建产物
#   .vault            测试库，不是源码

param(
    [string]$Source = 'D:\VideoVault',
    [string]$DestRoot = 'D:\VideoVault-backups',
    [int]$Keep = 5,
    [switch]$Quiet,
    # 当天已备份过也强制再跑一次
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

function Say($msg) {
    if (-not $Quiet) { Write-Output $msg }
}

if (-not (Test-Path $Source)) {
    Write-Error "源目录不存在：$Source"
    exit 2
}

$gitHead = Join-Path $Source '.git'
if (-not (Test-Path $gitHead)) {
    Write-Error "源目录里没有 .git，备份将不含版本历史：$Source"
    exit 3
}

New-Item -ItemType Directory -Force -Path $DestRoot | Out-Null

# 一天只备份一次：登录时触发的那次如果已经跑过（例如开机多次），直接跳过
if (-not $Force) {
    $lastLog = Join-Path $DestRoot '_logs\backup.log'
    if (Test-Path $lastLog) {
        $tail = Get-Content $lastLog -Tail 1 -ErrorAction SilentlyContinue
        if ($tail -match '^(\d{4}-\d{2}-\d{2})') {
            if ($matches[1] -eq (Get-Date -Format 'yyyy-MM-dd')) {
                Say "今天（$($matches[1])）已备份过，跳过。要强制再备一份加 -Force"
                exit 0
            }
        }
    }
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$dest = Join-Path $DestRoot $stamp
Say "备份 $Source -> $dest"

$excludeDirs = @(
    (Join-Path $Source 'src-tauri\target'),
    (Join-Path $Source 'node_modules'),
    (Join-Path $Source 'dist'),
    (Join-Path $Source '.vault'),
    (Join-Path $Source 'src-tauri\gen')
)

# robocopy 是增量复制，能绕过部分扇区读失败重试；退出码 0-7 正常，>=8 才算失败
robocopy $Source $dest /E /NFL /NDL /NJH /NJS /R:2 /W:1 /XD @excludeDirs | Out-Null
$code = $LASTEXITCODE
if ($code -ge 8) {
    Write-Error "robocopy 失败，退出码 $code"
    exit 4
}

# 校验：关键文件必须在，git 必须能读
$must = @('package.json', 'src\App.tsx', 'src-tauri\src\db.rs', '.git\HEAD')
$missing = @()
foreach ($m in $must) {
    if (-not (Test-Path (Join-Path $dest $m))) { $missing += $m }
}
if ($missing.Count -gt 0) {
    Write-Error ("备份不完整，缺失：" + ($missing -join ', '))
    exit 5
}

$head = & git -C $dest rev-parse --short HEAD 2>$null
if (-not $head) {
    Write-Error "备份里的 git 读不出 HEAD，可能损坏"
    exit 6
}

$size = (Get-ChildItem $dest -Recurse -File -Force -ErrorAction SilentlyContinue |
         Measure-Object Length -Sum).Sum
Say ("完成：HEAD={0}  体积={1}MB" -f $head, [math]::Round($size / 1MB, 1))

# 只留最近 N 份
$all = Get-ChildItem $DestRoot -Directory -ErrorAction SilentlyContinue |
       Where-Object { $_.Name -match '^\d{8}-\d{6}$' } |
       Sort-Object Name -Descending
if ($all.Count -gt $Keep) {
    $all | Select-Object -Skip $Keep | ForEach-Object {
        Say "清理旧备份：$($_.Name)"
        Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# 写一行日志，方便排查
$logDir = Join-Path $DestRoot '_logs'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$logLine = "{0}`t{1}`tHEAD={2}`t{3}MB" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $stamp, $head, [math]::Round($size / 1MB, 1)
Add-Content -Path (Join-Path $logDir 'backup.log') -Value $logLine -Encoding UTF8

exit 0
