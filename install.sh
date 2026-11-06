#!/usr/bin/env bash
# CCMS 一键安装：Linux / macOS / WSL。
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
  --uninstall    删除已安装的 ccms（配置目录保留）
  -h, --help     显示帮助

环境变量:
  PREFIX         等同 --prefix
  CCMS_PROXY     上游代理（默认直连，不读系统 http_proxy）
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

# Windows 的 bash 没有 /usr/local 语义，权限模型也不同，
# 统一交给 install.ps1（同一个仓库，逻辑只有一份）。
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

command -v cargo >/dev/null 2>&1 || die "需要 Rust 工具链，先安装：https://rustup.rs"
BIN_DIR="$PREFIX/bin"
BIN="$BIN_DIR/ccms"

if [[ $UNINSTALL -eq 1 ]]; then
  rm -f "$BIN"
  ok "已删除 $BIN（配置目录 ~/.config/ccms 保留）"
  exit 0
fi

printf '== 构建 ==\n'
cd "$ROOT"
if [[ -f Cargo.lock ]]; then
  cargo build --release --locked
else
  cargo build --release
fi

mkdir -p "$BIN_DIR"
cp -f "target/release/ccms" "$BIN"
chmod 755 "$BIN"
ok "已安装 $BIN"

printf '== 初始化配置 ==\n'
"$BIN" init

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
  if grep -qF '# >>> ccms >>>' "$PATH_RC"; then
    ok "$PATH_RC 里已有 ccms 的 PATH 配置"
  elif [[ "$PATH_RC" == *config.fish ]]; then
    printf '\n# >>> ccms >>>\nfish_add_path %s\n# <<< ccms <<<\n' "$BIN_DIR" >> "$PATH_RC"
    ok "已把 $BIN_DIR 写入 $PATH_RC（重开 shell 生效）"
  else
    printf '\n# >>> ccms >>>\nexport PATH="%s:$PATH"\n# <<< ccms <<<\n' "$BIN_DIR" >> "$PATH_RC"
    ok "已把 $BIN_DIR 写入 $PATH_RC（重开 shell 或 source 后生效）"
  fi
elif [[ $ON_PATH -eq 1 ]]; then
  ok "$BIN_DIR 已在 PATH 中"
fi

printf '\n'
ok "安装完成"
FOUND="$(command -v ccms 2>/dev/null || true)"
if [[ -n "$FOUND" && "$FOUND" != "$BIN" ]]; then
  printf '! PATH 里还有另一个 ccms: %s\n' "$FOUND"
  printf '  若那是 0.1 的 Node 版，建议删除它或把它从 PATH 移走\n'
fi
printf '下一步: ccms key <provider> sk-...  →  ccms on  →  新开一个 claude\n'
printf '验证:   ccms doctor && ./scripts/validate.sh\n'
