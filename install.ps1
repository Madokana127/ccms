#Requires -Version 5.1
<#
.SYNOPSIS
  Alkanet 一键安装（Windows 原生 PowerShell）。
.DESCRIPTION
  构建 Rust 守护进程 alkanetd.exe 与 Go CLI aknt.exe，安装到用户目录，
  写入用户 PATH，并初始化 config.toml。已有配置不会被覆盖。
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\install.ps1
#>
[CmdletBinding()]
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\alkanet'),
    [switch]$Uninstall,
    [switch]$NoPath
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$cliExe = Join-Path $Prefix 'aknt.exe'
$daemonExe = Join-Path $Prefix 'alkanetd.exe'
$configDir = Join-Path $env:USERPROFILE '.config\alkanet'

function Fail([string]$message) {
    Write-Host "✗ $message" -ForegroundColor Red
    exit 1
}
function Ok([string]$message) {
    Write-Host "✓ $message" -ForegroundColor Green
}

if ($Uninstall) {
    if (Test-Path $cliExe) { Remove-Item $cliExe -Force }
    if (Test-Path $daemonExe) { Remove-Item $daemonExe -Force }
    Ok "已删除 $cliExe 与 $daemonExe（配置目录 $configDir 保留）"
    exit 0
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Fail '需要 Rust 工具链，先安装：https://rustup.rs'
}
if (-not (Get-Command go -ErrorAction SilentlyContinue)) {
    Fail '需要 Go 1.22+ 工具链，先安装：https://go.dev/dl/'
}

Write-Host '== 构建 alkanetd (Rust) + aknt (Go) =='
Push-Location $root
try {
    if (Test-Path (Join-Path $root 'Cargo.lock')) {
        cargo build --release --locked --bin alkanetd
    } else {
        cargo build --release --bin alkanetd
    }
    if ($LASTEXITCODE -ne 0) { Fail 'cargo build 失败' }

    go build -trimpath -ldflags="-s -w" -o "target\release\aknt.exe" .\cmd\aknt
    if ($LASTEXITCODE -ne 0) { Fail 'go build 失败' }
} finally {
    Pop-Location
}

New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
$builtDaemon = Join-Path $root 'target\release\alkanetd.exe'
$builtCli = Join-Path $root 'target\release\aknt.exe'
if (-not (Test-Path $builtDaemon)) { Fail "找不到构建产物 $builtDaemon" }
if (-not (Test-Path $builtCli)) { Fail "找不到构建产物 $builtCli" }
Copy-Item $builtDaemon $daemonExe -Force
Copy-Item $builtCli $cliExe -Force
Ok "已安装 $cliExe 与 $daemonExe"

if (-not $NoPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }
    if ($userPath -notlike "*$Prefix*") {
        $newPath = $userPath.TrimEnd(';') + ';' + $Prefix
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Ok '已加入用户 PATH（新终端生效）'
    } else {
        Ok '用户 PATH 已包含该目录'
    }
}
$env:Path = "$Prefix;$env:Path"

Write-Host '== 初始化配置 =='
& $cliExe config path | Out-Null
if ($LASTEXITCODE -ne 0) { Fail 'aknt config path 初始化失败' }

Write-Host ''
Ok '安装完成'
Write-Host '下一步: aknt key add <provider> stdin  →  aknt on  →  新开一个 claude'
Write-Host '验证:   aknt status check'
