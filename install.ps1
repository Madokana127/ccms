#Requires -Version 5.1
<#
.SYNOPSIS
  CCMS 一键安装（Windows 原生 PowerShell）。
.DESCRIPTION
  使用 cargo 构建 release 二进制，安装到用户目录，写入用户 PATH，
  并初始化配置。已有配置不会被覆盖。
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\install.ps1
#>
[CmdletBinding()]
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\ccms'),
    [switch]$Uninstall,
    [switch]$NoPath
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$exe = Join-Path $Prefix 'ccms.exe'
$configDir = Join-Path $env:USERPROFILE '.config\ccms'

function Fail([string]$message) {
    Write-Host "✗ $message" -ForegroundColor Red
    exit 1
}
function Ok([string]$message) {
    Write-Host "✓ $message" -ForegroundColor Green
}

if ($Uninstall) {
    if (Test-Path $exe) { Remove-Item $exe -Force }
    Ok "已删除 $exe（配置目录 $configDir 保留）"
    exit 0
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Fail '需要 Rust 工具链，先安装：https://rustup.rs'
}

Write-Host '== 构建 =='
Push-Location $root
try {
    if (Test-Path (Join-Path $root 'Cargo.lock')) {
        cargo build --release --locked
    } else {
        cargo build --release
    }
    if ($LASTEXITCODE -ne 0) { Fail 'cargo build 失败' }
} finally {
    Pop-Location
}

New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
$built = Join-Path $root 'target\release\ccms.exe'
if (-not (Test-Path $built)) { Fail "找不到构建产物 $built" }
Copy-Item $built $exe -Force
Ok "已安装 $exe"

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
& $exe --init
if ($LASTEXITCODE -ne 0) { Fail 'ccms --init 失败' }

Write-Host ''
Ok '安装完成'
Write-Host '下一步: ccms --key <provider> sk-...  →  ccms --on  →  新开一个 claude'
Write-Host '验证:   ccms --doctor'
