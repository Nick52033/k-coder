# scripts/clean-build-cache.ps1 — 回收 Rust 构建缓存占用
#
# 背景：本仓库依赖图很大（tauri / ort / chromiumoxide / rusqlite(bundled) /
# pdf-extract / image …），cargo 会留下三类体积巨大的中间产物：
#   1. target/<profile>/deps/.tmp*.temp-archive/  链接中断残留的临时归档（可达数 GB）
#   2. target/validation-*/                       用 --target-dir 做隔离验证后遗留的独立 target
#   3. target/debug/incremental/                  增量编译会话缓存（可达数十 GB）
# 本脚本按“可安全删除”的顺序回收它们，默认不碰 debug/release 的正常产物。
#
# 用法：
#   pnpm clean:cache
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/clean-build-cache.ps1
#   ... -IncludeRelease                          # 连 debug/release 正常产物一起清（下次全量重编译）
#   ... -DeployDir "D:\apps\k-coder"             # 顺带瘦身部署目录（清 deps/build 等中间件）

param(
    [switch]$IncludeRelease,
    [string]$DeployDir
)

$ErrorActionPreference = "Stop"
$targetRoot = Join-Path $PSScriptRoot "..\src-tauri\target"

function Get-SizeMB {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $item = Get-Item -LiteralPath $Path -Force
    if (-not $item.PSIsContainer) { return [math]::Round($item.Length / 1MB, 1) }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return 0 }
    return [math]::Round($sum / 1MB, 1)
}

function Remove-Junk {
    param([string]$Path, [string]$Label)
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $before = Get-SizeMB -Path $Path
    try {
        if ((Get-Item -LiteralPath $Path -Force).PSIsContainer) {
            [System.IO.Directory]::Delete($Path, $true)
        } else {
            [System.IO.File]::Delete($Path)
        }
        Write-Host ("  已删除 {0,-40} {1,9} MB" -f $Label, $before) -ForegroundColor DarkGray
        return $before
    } catch {
        # .NET 在个别目录上可能因残留句柄/长路径失败，退回 Remove-Item，仍失败则只告警不中断
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
            Write-Host ("  已删除 {0,-40} {1,9} MB" -f $Label, $before) -ForegroundColor DarkGray
            return $before
        } catch {
            Write-Host ("  跳过   {0,-40} {1}" -f $Label, $_.Exception.Message) -ForegroundColor Yellow
            return 0
        }
    }
}

$freed = 0

Write-Host "== 1/4 链接中断残留的临时归档 ==" -ForegroundColor Cyan
foreach ($profile in @("debug", "release")) {
    $deps = Join-Path $targetRoot "$profile\deps"
    if (Test-Path -LiteralPath $deps) {
        Get-ChildItem -LiteralPath $deps -Directory -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like ".tmp*.temp-archive" } |
            ForEach-Object { $freed += Remove-Junk -Path $_.FullName -Label $_.Name }
    }
}

Write-Host "== 2/4 隔离验证遗留的 target 目录 ==" -ForegroundColor Cyan
Get-ChildItem -LiteralPath $targetRoot -Directory -Force -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like "validation-*" } |
    ForEach-Object { $freed += Remove-Junk -Path $_.FullName -Label $_.Name }

Write-Host "== 3/4 增量编译缓存 ==" -ForegroundColor Cyan
$freed += Remove-Junk -Path (Join-Path $targetRoot "debug\incremental") -Label "debug\incremental"

if ($IncludeRelease) {
    Write-Host "== 4/4 正常构建产物（-IncludeRelease）==" -ForegroundColor Cyan
    $freed += Remove-Junk -Path (Join-Path $targetRoot "debug") -Label "debug"
    $freed += Remove-Junk -Path (Join-Path $targetRoot "release") -Label "release"
} else {
    Write-Host "== 4/4 debug/examples ==" -ForegroundColor Cyan
    $freed += Remove-Junk -Path (Join-Path $targetRoot "debug\examples") -Label "debug\examples"
}

if ($DeployDir) {
    Write-Host "== 顺带瘦身部署目录 $DeployDir ==" -ForegroundColor Cyan
    foreach ($name in @("deps", "build", "incremental", ".fingerprint", "examples", "k_coder.pdb", "k-coder.d")) {
        $freed += Remove-Junk -Path (Join-Path $DeployDir $name) -Label $name
    }
}

Write-Host ""
Write-Host ("合计回收: {0:N2} GB" -f ($freed / 1024)) -ForegroundColor Green
if (-not $IncludeRelease) {
    Write-Host "提示：incremental 缓存删除后，下一次 debug 编译会慢一些，属预期。" -ForegroundColor Yellow
}
