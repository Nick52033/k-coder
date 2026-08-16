# scripts/deploy.ps1 — 构建后自动部署（可移植版）
# 流程：结束正在运行的 k-coder -> 增量同步到目标目录 -> 重新启动 k-coder
# 关键设计：脚本会把真正的部署动作转交给一个独立进程（detached），
#           避免自身作为 k-coder 的子进程被连带终止。
param(
    [string]$Dest,
    [switch]$NoRestart,   # 部署后不重新打开 k-coder
    [switch]$Worker       # 内部标记：表示当前已是独立 worker 进程，直接干活
)

$ErrorActionPreference = "Stop"

# ============================================================
# 入口：若非 worker 进程，则派生独立进程执行真正的部署逻辑后立即返回
# ============================================================
if (-not $Worker) {
    $argList = @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-File", "`"$($MyInvocation.MyCommand.Path)`"",
        "-Worker"
    )
    if ($Dest) { $argList += @("-Dest", "`"$Dest`"") }
    if ($NoRestart) { $argList += "-NoRestart" }

    # 独立启动一个 powershell 进程，隐藏窗口，不等待它结束
    Start-Process -FilePath "powershell.exe" -ArgumentList $argList -WindowStyle Hidden
    Write-Host "部署任务已转交后台独立进程执行。" -ForegroundColor Cyan
    exit 0
}

# ============================================================
# 以下为 worker 进程逻辑（独立于 k-coder 进程树）
# ============================================================

# 源目录基于脚本位置自动推导，任何机器 clone 后都能用
$source = Join-Path $PSScriptRoot "..\src-tauri\target\release"

# 目标目录优先级：命令行参数 > 环境变量 > 本地配置文件 > 提示
if (-not $Dest) { $Dest = $env:KC_DEPLOY_DEST }
if (-not $Dest -and (Test-Path (Join-Path $PSScriptRoot "deploy.config.json"))) {
    $cfg = Get-Content (Join-Path $PSScriptRoot "deploy.config.json") -Raw | ConvertFrom-Json
    $Dest = $cfg.dest
}
if (-not $Dest) {
    Write-Host "未指定部署目录，用法：" -ForegroundColor Yellow
    Write-Host '  powershell -File scripts/deploy.ps1 -Dest "E:\Program Files\k-coder"'
    Write-Host "  或设置环境变量 KC_DEPLOY_DEST"
    exit 1
}

# 1. 程序在运行则先结束进程，避免文件占用
if (Get-Process -Name "k-coder" -ErrorAction SilentlyContinue) {
    Get-Process -Name "k-coder" -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}

# 2. 确保目标目录存在
New-Item -ItemType Directory -Force -Path $Dest | Out-Null

# 3. 增量镜像同步（robocopy 退出码 0-7 均成功，>=8 为错误）
robocopy $source $Dest /MIR /NFL /NDL /NJH /NJS /NP
if ($LASTEXITCODE -ge 8) {
    Write-Error "robocopy 失败，退出码: $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host "部署完成: $source -> $Dest" -ForegroundColor Green

# 4. 重新打开 k-coder（除非指定 NoRestart）
$exe = Join-Path $Dest "k-coder.exe"
if (-not $NoRestart) {
    if (Test-Path $exe) {
        Start-Process -FilePath $exe
        Write-Host "已重新启动 k-coder" -ForegroundColor Green
    } else {
        Write-Host "未找到 $exe，跳过重启" -ForegroundColor Yellow
    }
}
