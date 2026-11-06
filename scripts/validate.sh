#!/usr/bin/env bash
# 隔离验证：临时 CCMS_HOME，不动真实配置，不访问外网。
# 用法: ./scripts/validate.sh    （可用 CCMS_BIN 指定二进制）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${CCMS_BIN:-}"
if [[ -z "$BIN" ]]; then
  if [[ -x "$ROOT/target/release/ccms" ]]; then
    BIN="$ROOT/target/release/ccms"
  elif command -v ccms >/dev/null 2>&1; then
    BIN="$(command -v ccms)"
  else
    printf '✗ 找不到 ccms，先 ./install.sh 或 cargo build --release\n' >&2
    exit 1
  fi
fi

TMP="$(mktemp -d)"
PORT="${CCMS_TEST_PORT:-18788}"
export CCMS_HOME="$TMP/ccms"
export CLAUDE_SETTINGS="$TMP/.claude/settings.json"
export NO_COLOR=1
FAIL=0

cleanup() {
  "$BIN" router stop >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

ok()  { printf '✓ %s\n' "$1"; }
bad() { printf '✗ %s\n' "$1"; FAIL=1; }

expect() { # 期望片段 实际输出 说明
  if [[ "$2" == *"$1"* ]]; then ok "$3"; else bad "$3（输出里没有「$1」）"; fi
}

refute() {
  if [[ "$2" == *"$1"* ]]; then bad "$3（仍包含「$1」）"; else ok "$3"; fi
}

perms() {
  if stat -c %a "$1" >/dev/null 2>&1; then stat -c %a "$1"; else stat -f %Lp "$1"; fi
}

mkdir -p "$TMP/.claude" "$CCMS_HOME"

# 用测试端口替换默认监听，避免和真实 router 抢 8788。
"$BIN" init >/dev/null
sed "s/127.0.0.1:8788/127.0.0.1:$PORT/" "$CCMS_HOME/routes.json" > "$CCMS_HOME/routes.tmp"
mv "$CCMS_HOME/routes.tmp" "$CCMS_HOME/routes.json"

expect "0.2" "$("$BIN" --version)" "版本"
expect "路由表" "$("$BIN" --route --list)" "旧写法 --route 映射"
expect "Sonnet" "$("$BIN")" "首页渲染"

# ---------- 密钥池 + 加密 ----------
"$BIN" key kimi sk-test-alpha >/dev/null
"$BIN" key kimi sk-test-beta >/dev/null
expect "sk-tes" "$("$BIN" key kimi list)" "密钥掩码展示"
COUNT="$("$BIN" key kimi list | grep -c '…' || true)"
[[ "$COUNT" == "2" ]] && ok "密钥池计数 = 2" || bad "密钥池计数应为 2，实际 $COUNT"

[[ -f "$CCMS_HOME/keys.enc" ]] && ok "已生成 keys.enc" || bad "缺少 keys.enc"
[[ -f "$CCMS_HOME/keystore.key" ]] && ok "已生成 keystore.key" || bad "缺少 keystore.key"
[[ "$(perms "$CCMS_HOME/keys.enc")" == "600" ]] && ok "keys.enc 权限 600" || bad "keys.enc 权限不是 600"
[[ "$(perms "$CCMS_HOME/keystore.key")" == "600" ]] && ok "keystore.key 权限 600" || bad "keystore.key 权限不是 600"
expect "aes-256-gcm" "$("$BIN" vault status)" "密钥库状态"
expect "可解密      是" "$("$BIN" vault status)" "密钥库可解密"
refute "sk-test-alpha" "$(cat "$CCMS_HOME/keys.enc")" "密文里没有明文密钥"

"$BIN" key kimi remove 1 >/dev/null
COUNT="$("$BIN" key kimi list | grep -c '…' || true)"
[[ "$COUNT" == "1" ]] && ok "remove 生效" || bad "remove 未生效（$COUNT）"
"$BIN" key kimi clear >/dev/null
expect "为空" "$("$BIN" key kimi list)" "clear 生效"

# ---------- 路由 ----------
"$BIN" route set haiku deepseek deepseek-flash >/dev/null
expect "deepseek/deepseek-flash" "$("$BIN" route list)" "route set"
"$BIN" route all kimi >/dev/null
expect "kimi/kimi-k3.1-pro" "$("$BIN" route list)" "route all"
if "$BIN" route set nope kimi x >/dev/null 2>&1; then
  bad "非法档位应被拒绝"
else
  ok "非法档位被拒绝"
fi
if "$BIN" route set opus ghost model >/dev/null 2>&1; then
  bad "未知 provider 应被拒绝"
else
  ok "未知 provider 被拒绝"
fi

# ---------- 模式切换 + router ----------
"$BIN" on >/dev/null
SETTINGS="$(cat "$CLAUDE_SETTINGS")"
expect "http://127.0.0.1:$PORT" "$SETTINGS" "settings.json 写入 base_url"
expect "ccms-subagent" "$SETTINGS" "settings.json 写入 subagent 档位"
expect "ccms-haiku" "$SETTINGS" "settings.json 写入 haiku 档位"
expect "运行中" "$("$BIN" router status)" "router 运行中"
expect '"router_alive": true' "$("$BIN" status --json)" "status --json"
expect "ccms-opus" "$(curl -sS --noproxy '*' "http://127.0.0.1:$PORT/v1/models")" "/v1/models 返回哨兵"

"$BIN" router restart >/dev/null
expect "运行中" "$("$BIN" router status)" "router 重启"
"$BIN" router stop >/dev/null
expect "未运行" "$("$BIN" router status)" "router 停止"

"$BIN" off >/dev/null
refute "ANTHROPIC_BASE_URL" "$(cat "$CLAUDE_SETTINGS")" "off 清理 env"

expect "暂无用量记录" "$("$BIN" stats)" "统计空态"
expect "明文" "$(rm -f "$CCMS_HOME/keys.enc" "$CCMS_HOME/keystore.key"; "$BIN" vault status)" "无密钥库时的提示"

printf '\n'
if [[ $FAIL -eq 0 ]]; then
  ok "全部验证通过（隔离目录 $TMP）"
else
  printf '✗ 存在失败项\n' >&2
  exit 1
fi
