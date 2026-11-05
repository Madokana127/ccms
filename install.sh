#!/usr/bin/env bash
# 将本仓库安装到当前 Linux/WSL 环境。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-/usr/local}"
CCX_HOME="${CCX_HOME:-$HOME/.config/ccx}"

die() { printf '✗ %s\n' "$*" >&2; exit 1; }
ok()  { printf '✓ %s\n' "$*"; }

command -v node >/dev/null || die "缺少 node"
command -v jq   >/dev/null || die "缺少 jq"
command -v curl >/dev/null || die "缺少 curl"
[[ $EUID -eq 0 ]] || die "需要 root 权限，请用 sudo ./install.sh"

bash -n "$ROOT/bin/ccx"
node --check "$ROOT/lib/ccx/router.js"
node --check "$ROOT/lib/ccx/stats.js"
jq -e . "$ROOT/config/routes.json" >/dev/null
jq -e . "$ROOT/config/providers.json" >/dev/null

install -d "$PREFIX/bin" "$PREFIX/lib/ccx" "$CCX_HOME"
install -m 755 "$ROOT/bin/ccx" "$PREFIX/bin/ccx"
install -m 644 "$ROOT/lib/ccx/router.js" "$PREFIX/lib/ccx/router.js"
install -m 644 "$ROOT/lib/ccx/stats.js" "$PREFIX/lib/ccx/stats.js"
ln -sfn "$PREFIX/bin/ccx" "$PREFIX/bin/ccms"

# 已有配置永不覆盖，避免部署时丢路由或密钥。
if [[ ! -f "$CCX_HOME/routes.json" ]]; then
  install -m 600 "$ROOT/config/routes.json" "$CCX_HOME/routes.json"
fi
if [[ ! -f "$CCX_HOME/providers.json" ]]; then
  install -m 644 "$ROOT/config/providers.json" "$CCX_HOME/providers.json"
fi
if [[ ! -f "$CCX_HOME/keys.env" ]]; then
  install -m 600 "$ROOT/config/keys.env.example" "$CCX_HOME/keys.env"
fi

# README 直接链接到源码，避免安装目录保留文档副本。
ln -sfn "$ROOT/README.md" "$CCX_HOME/README.md"

ok "已部署到 $PREFIX"
ok "配置目录 $CCX_HOME（现有配置未覆盖）"
if curl -fsS -m 2 "http://$(jq -r '.listen' "$CCX_HOME/routes.json")/ccx/health" >/dev/null 2>&1; then
  printf '! router 正在运行；如需加载新代码，执行: ccx --router --restart\n'
fi
