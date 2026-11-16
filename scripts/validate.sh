#!/usr/bin/env bash
# Alkanet 隔离验证：临时 ALKANET_HOME，不动真实配置，不访问外网。
# 用法: ./scripts/validate.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI="${AKNT_BIN:-$ROOT/target/release/aknt}"
DAEMON="${ALKANETD_BIN:-$ROOT/target/release/alkanetd}"

[[ -x "$CLI" ]] || { printf '✗ 找不到 aknt: %s\n' "$CLI" >&2; exit 1; }
[[ -x "$DAEMON" ]] || { printf '✗ 找不到 alkanetd: %s\n' "$DAEMON" >&2; exit 1; }

TMP="$(mktemp -d)"
PORT=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  PORT="$((20000 + RANDOM % 20000))"
  if ! (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
    break
  fi
done

export ALKANET_HOME="$TMP/alkanet"
export CLAUDE_SETTINGS="$TMP/.claude/settings.json"
export ALKANETD_BIN="$DAEMON"
export NO_COLOR=1
FAIL=0

cleanup() {
  "$CLI" router stop >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

ok()     { printf '✓ %s\n' "$1"; }
bad()    { printf '✗ %s\n' "$1"; FAIL=1; }
expect() { [[ "$2" == *"$1"* ]] && ok "$3" || bad "$3（缺少「$1」）"; }
refute() { [[ "$2" == *"$1"* ]] && bad "$3（仍含「$1」）" || ok "$3"; }
perms()  { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"; }

mkdir -p "$TMP/.claude" "$ALKANET_HOME"

# ---------- 版本 / 帮助 / 看板 / 配置 ----------
expect "aknt 1.0.0" "$("$CLI" version)" "version 输出"
expect "Available Commands" "$("$CLI" --help)" "--help 输出"

DASH="$("$CLI")"
expect "Mode " "$(printf '%s\n' "$DASH" | head -n1)" "看板第一行 Mode"
expect "Route List" "$DASH" "看板路由段"
expect "提示  aknt --help" "$DASH" "看板帮助提示"
[[ -f "$ALKANET_HOME/config.toml" ]] && ok "自动生成 config.toml" || bad "未生成 config.toml"
[[ "$(perms "$ALKANET_HOME/config.toml")" == "600" ]] && ok "config.toml 权限 600" || bad "config.toml 权限不是 600"

# 替换端口避免冲突
sed "s/127.0.0.1:8788/127.0.0.1:$PORT/" "$ALKANET_HOME/config.toml" > "$ALKANET_HOME/config.tmp"
mv "$ALKANET_HOME/config.tmp" "$ALKANET_HOME/config.toml"

# ---------- 密钥池 + 加密 ----------
"$CLI" key add kimi sk-test-alpha-1234 >/dev/null
printf 'sk-test-beta-5678\n' | "$CLI" key add kimi --stdin >/dev/null
LIST="$("$CLI" key ls kimi)"
expect "sk-tes" "$LIST" "key ls 掩码"
expect "共 2 把" "$LIST" "key 池计数 = 2"
[[ "$(perms "$ALKANET_HOME/keys.enc")" == "600" ]] && ok "keys.enc 权限 600" || bad "keys.enc 权限不是 600"
[[ "$(perms "$ALKANET_HOME/keystore.key")" == "600" ]] && ok "keystore.key 权限 600" || bad "keystore.key 权限不是 600"
refute "sk-test-alpha-1234" "$(cat "$ALKANET_HOME/keys.enc")" "密文无明文"

"$CLI" key rm kimi 1 >/dev/null
expect "共 1 把" "$("$CLI" key ls kimi)" "key rm 生效"
"$CLI" key clear kimi >/dev/null
expect "为空" "$("$CLI" key ls kimi)" "key clear 生效"

# ---------- use / config ----------
"$CLI" use deepseek deepseek-v4-pro >/dev/null
expect "deepseek/deepseek-v4-pro" "$("$CLI" config)" "use 更新 config.toml"

# ---------- on / router / off ----------
printf '%s\n' '{"env":{"ANTHROPIC_API_KEY":"sk-official","FOO":"keep"}}' > "$CLAUDE_SETTINGS"
"$CLI" on >/dev/null
SETTINGS="$(cat "$CLAUDE_SETTINGS")"
expect "http://127.0.0.1:$PORT" "$SETTINGS" "on 写入 base_url"
expect "alkanet-sonnet" "$SETTINGS" "on 写入哨兵模型"
refute "sk-official" "$SETTINGS" "on 替换官方 key"
expect '"router_alive": true' "$("$CLI" status --json)" "status --json"

HEALTH="$(curl -sS --noproxy '*' "http://127.0.0.1:$PORT/alkanet/health")"
expect '"service":"alkanet"' "$HEALTH" "health service=alkanet"

"$CLI" router restart >/dev/null
expect "运行中" "$("$CLI" status)" "router restart"
"$CLI" router stop >/dev/null
expect "未运行" "$("$CLI" status)" "router stop"

"$CLI" off >/dev/null
SETTINGS="$(cat "$CLAUDE_SETTINGS")"
expect "sk-official" "$SETTINGS" "off 恢复官方 key"
expect "FOO" "$SETTINGS" "off 保留未受管 env"
refute "ANTHROPIC_BASE_URL" "$SETTINGS" "off 清理受管 env"

# ---------- stats ----------
expect "暂无用量记录" "$("$CLI" stats)" "stats 空态"
expect '"days"' "$("$CLI" stats --json)" "stats --json"

printf '\n'
if [[ $FAIL -eq 0 ]]; then
  ok "全部验证通过（隔离目录 $TMP）"
else
  printf '✗ 存在失败项\n' >&2
  exit 1
fi
