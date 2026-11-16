#!/usr/bin/env bash
# Alkanet 一键安装：Linux / macOS / WSL。
# Windows 原生环境（git-bash / MSYS）会自动转交 install.ps1。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
PREFIX_SET=0
ADD_PATH=1
UNINSTALL=0

usage() {
  cat <<'EOF'
用法: ./install.sh [选项]

  --prefix DIR   安装到 DIR/bin（默认 ~/.local）
  --no-path      不修改 shell 启动文件
  --uninstall    删除已安装的 aknt 与 alkanetd（配置目录保留）
  -h, --help     显示帮助

环境变量:
  PREFIX         等同 --prefix
  ALKANET_PROXY  上游代理（默认直连，不读系统 http_proxy）
EOF
}

die() { printf '✗ %s\n' "$*" >&2; exit 1; }
ok()  { printf '✓ %s\n' "$*"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)    PREFIX="${2:?--prefix 需要目录}"; PREFIX_SET=1; shift 2 ;;
    --prefix=*)  PREFIX="${1#*=}"; PREFIX_SET=1; shift ;;
    --no-path)   ADD_PATH=0; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help)   usage; exit 0 ;;
    *) printf '未知参数: %s\n\n' "$1" >&2; usage; exit 1 ;;
  esac
done

OS="$(uname -s)"
case "$OS" in
  MINGW*|MSYS*|CYGWIN*)
    command -v powershell.exe >/dev/null 2>&1 || die "找不到 powershell.exe"
    WIN_ROOT="$(cd "$ROOT" && pwd -W 2>/dev/null || printf '%s' "$ROOT")"
    PS_ARGS=()
    [[ $UNINSTALL -eq 1 ]] && PS_ARGS+=(-Uninstall)
    [[ $ADD_PATH -eq 0 ]] && PS_ARGS+=(-NoPath)
    [[ $PREFIX_SET -eq 1 ]] && PS_ARGS+=(-Prefix "$PREFIX")
    exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$WIN_ROOT\\install.ps1" "${PS_ARGS[@]}"
    ;;
  Linux|Darwin) ;;
  *) die "不支持的平台: $OS（Linux / macOS / Windows 的 git-bash）" ;;
esac

BIN_DIR="$PREFIX/bin"
CLI_BIN="$BIN_DIR/aknt"
DAEMON_BIN="$BIN_DIR/alkanetd"

if [[ $UNINSTALL -eq 1 ]]; then
  rm -f "$CLI_BIN" "$DAEMON_BIN"
  ok "已删除 $CLI_BIN 与 $DAEMON_BIN（配置目录 ~/.config/alkanet 保留）"
  exit 0
fi

command -v cargo >/dev/null 2>&1 || die "需要 Rust 工具链，先安装：https://rustup.rs"
command -v go >/dev/null 2>&1 || die "需要 Go 1.22+ 工具链，先安装：https://go.dev/dl/"

printf '== 构建 alkanetd (Rust) + aknt (Go) ==\n'
cd "$ROOT"
if [[ -f Cargo.lock ]]; then
  cargo build --release --locked --bin alkanetd
else
  cargo build --release --bin alkanetd
fi
go build -trimpath -ldflags="-s -w" -o "target/release/aknt" ./cmd/aknt

mkdir -p "$BIN_DIR"
cp -f "target/release/alkanetd" "$DAEMON_BIN"
cp -f "target/release/aknt" "$CLI_BIN"
chmod 755 "$DAEMON_BIN" "$CLI_BIN"
ok "已安装 $CLI_BIN 与 $DAEMON_BIN"

printf '== 初始化配置 ==\n'
"$CLI_BIN" config path >/dev/null

PATH_RC=""
case "$(basename "${SHELL:-bash}")" in
  zsh)  PATH_RC="$HOME/.zshrc" ;;
  fish) PATH_RC="$HOME/.config/fish/config.fish" ;;
  *)    PATH_RC="$HOME/.bashrc" ;;
esac

case ":$PATH:" in
  *":$BIN_DIR:"*) ON_PATH=1 ;;
  *) ON_PATH=0 ;;
esac

if [[ $ADD_PATH -eq 1 && $ON_PATH -eq 0 ]]; then
  mkdir -p "$(dirname "$PATH_RC")"
  touch "$PATH_RC"
  if grep -qF '# >>> alkanet >>>' "$PATH_RC"; then
    ok "$PATH_RC 里已有 alkanet 的 PATH 配置"
  elif [[ "$PATH_RC" == *config.fish ]]; then
    printf '\n# >>> alkanet >>>\nfish_add_path %s\n# <<< alkanet <<<\n' "$BIN_DIR" >> "$PATH_RC"
    ok "已把 $BIN_DIR 写入 $PATH_RC（重开 shell 生效）"
  else
    printf '\n# >>> alkanet >>>\nexport PATH="%s:$PATH"\n# <<< alkanet <<<\n' "$BIN_DIR" >> "$PATH_RC"
    ok "已把 $BIN_DIR 写入 $PATH_RC（重开 shell 或 source 后生效）"
  fi
elif [[ $ON_PATH -eq 1 ]]; then
  ok "$BIN_DIR 已在 PATH 中"
fi

printf '\n'
ok "安装完成"
printf '下一步: aknt key add <provider> --stdin  →  aknt on  →  新开一个 claude\n'
printf '验证:   aknt status --check && ./scripts/validate.sh\n'
