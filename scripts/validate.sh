#!/usr/bin/env bash
# 在 WSL 中直接验证 Windows 仓库，不复制源码、不影响现有配置和 router。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${CCX_TEST_PORT:-18788}"
TMP="$(mktemp -d)"
PID=""

cleanup() {
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

die() { printf '✗ %s\n' "$*" >&2; exit 1; }
ok()  { printf '✓ %s\n' "$*"; }

for cmd in node jq curl; do
  command -v "$cmd" >/dev/null || die "缺少 $cmd"
done

bash -n "$ROOT/bin/ccx"
node --check "$ROOT/lib/ccx/router.js"
node --check "$ROOT/lib/ccx/stats.js"
jq -e . "$ROOT/config/routes.json" >/dev/null
jq -e . "$ROOT/config/providers.json" >/dev/null
ok "语法与配置 JSON 校验通过"

mkdir -p "$TMP/.config/ccx" "$TMP/.claude"
jq --argjson port "$PORT" '.listen = ("127.0.0.1:" + ($port|tostring))' \
  "$ROOT/config/routes.json" > "$TMP/.config/ccx/routes.json"
cp "$ROOT/config/providers.json" "$TMP/.config/ccx/providers.json"
: > "$TMP/.config/ccx/keys.env"
printf '{}\n' > "$TMP/.claude/settings.json"

run_ccx() {
  HOME="$TMP" \
  CCX_HOME="$TMP/.config/ccx" \
  CCX_ROUTER_JS="$ROOT/lib/ccx/router.js" \
  CCX_STATS_JS="$ROOT/lib/ccx/stats.js" \
  CLAUDE_SETTINGS="$TMP/.claude/settings.json" \
  NO_COLOR=1 bash "$ROOT/bin/ccx" "$@"
}

[[ "$(run_ccx --version)" == *"0.1.0"* ]] || die "ccx --version 输出异常"
run_ccx --route --list >/dev/null
run_ccx >/dev/null
ok "CLI 冒烟测试通过"

HOME="$TMP" node "$ROOT/lib/ccx/router.js" >"$TMP/router.log" 2>&1 &
PID=$!

for _ in $(seq 1 30); do
  sleep 0.1
  if curl -fsS -m 1 "http://127.0.0.1:$PORT/ccx/health" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$PID" 2>/dev/null || { cat "$TMP/router.log"; die "router 启动失败"; }
done

curl -fsS -m 2 "http://127.0.0.1:$PORT/ccx/health" | jq -e '.ok == true' >/dev/null \
  || die "router 健康检查失败"
run_ccx --router --status >/dev/null
ok "router 隔离启动与健康检查通过（127.0.0.1:$PORT）"

kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
PID=""
ok "全部验证通过"
