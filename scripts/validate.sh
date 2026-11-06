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
    printf '✗ 找不到 ccms，先 cargo build --release\n' >&2
    exit 1
  fi
fi

TMP="$(mktemp -d)"
PORT="${CCMS_TEST_PORT:-}"
if [[ -z "$PORT" ]]; then
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    PORT="$((20000 + RANDOM % 20000))"
    if ! (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
      break
    fi
  done
fi
export CCMS_HOME="$TMP/ccms"
export CLAUDE_SETTINGS="$TMP/.claude/settings.json"
export NO_COLOR=1
FAIL=0

cleanup() {
  "$BIN" --router --stop >/dev/null 2>&1 || true
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

equals() { # 期望 实际 说明
  if [[ "$2" == "$1" ]]; then ok "$3"; else bad "$3（期望「$1」，实际「$2」）"; fi
}

perms() {
  if stat -c %a "$1" >/dev/null 2>&1; then stat -c %a "$1"; else stat -f %Lp "$1"; fi
}

expect_fail() { # 说明 命令...
  local desc="$1"; shift
  local out
  if out="$("$@" 2>&1 >/dev/null)"; then
    bad "$desc（应非零退出）"
  else
    ok "$desc"
    EXPECT_FAIL_OUT="$out"
  fi
}

mkdir -p "$TMP/.claude" "$CCMS_HOME"

# 用测试端口替换默认监听，避免和真实 router 抢 8788。
"$BIN" --init >/dev/null
sed "s/127.0.0.1:8788/127.0.0.1:$PORT/" "$CCMS_HOME/routes.json" > "$CCMS_HOME/routes.tmp"
mv "$CCMS_HOME/routes.tmp" "$CCMS_HOME/routes.json"

# ---------- 版本 / 帮助 / 首页 ----------
equals "CCMS 0.2.0 — Claude Code Model Switcher" "$("$BIN" -v)" "旧式 -v"
equals "CCMS 0.2.0 — Claude Code Model Switcher" "$("$BIN" --version)" "旧式 --version"

for hflag in -h --help; do
  HELP="$("$BIN" "$hflag")"
  expect "CCMS — Claude Code Model Switcher  v0.2.0" "$HELP" "$hflag 旧式头部"
  expect "适用于Claude Code的Provider自由切换工具" "$HELP" "$hflag 旧 tagline"
  expect "ccms [--命令/-x] [参数]" "$HELP" "$hflag 旧式用法"
  refute "Commands:" "$HELP" "$hflag 不显示 clap Commands"
  refute "Usage:" "$HELP" "$hflag 不显示 clap Usage"
  refute "-V" "$HELP" "$hflag 不显示大写 -V"
  for flag in --on/-a --off/-o --status --list/-l --key/-k --stdin --vault --models/-m \
              --test/-t --doctor/-d --stats/-u --route/-r --router --init \
              --help/-h --version/-v "remove <n>" --by; do
    expect "$flag" "$HELP" "$hflag 列出 $flag"
  done
done

DASH="$("$BIN")"
expect "Mode " "$(printf '%s\n' "$DASH" | head -n1)" "首页第一行 Mode"
expect "Default " "$(printf '%s\n' "$DASH" | sed -n 2p)" "首页第二行 Default"
equals "Route List" "$(printf '%s\n' "$DASH" | sed -n 3p)" "首页第三行 Route List"
for tier in Fable Opus Sonnet Haiku subagent; do
  expect "$tier" "$DASH" "首页包含档位 $tier"
done
expect "-> " "$DASH" "首页箭头为 ->"
refute "→" "$DASH" "首页不使用 →"
equals "提示  ccms --help/-h" "$(printf '%s\n' "$DASH" | tail -n1)" "首页结尾帮助提示"
refute $'\033' "$DASH" "首页无 ANSI"

# ---------- 密钥池 + 加密 ----------
"$BIN" key kimi sk-test-alpha >/dev/null
"$BIN" --key kimi sk-test-beta >/dev/null
expect "sk-tes" "$("$BIN" key kimi list)" "密钥掩码展示"
expect "sk-tes" "$("$BIN" --key kimi list)" "旧式 --key list"
COUNT="$("$BIN" key kimi list | grep -c '…' || true)"
[[ "$COUNT" == "2" ]] && ok "密钥池计数 = 2" || bad "密钥池计数应为 2，实际 $COUNT"

[[ -f "$CCMS_HOME/keys.enc" ]] && ok "已生成 keys.enc" || bad "缺少 keys.enc"
[[ -f "$CCMS_HOME/keystore.key" ]] && ok "已生成 keystore.key" || bad "缺少 keystore.key"
[[ "$(perms "$CCMS_HOME/keys.enc")" == "600" ]] && ok "keys.enc 权限 600" || bad "keys.enc 权限不是 600"
[[ "$(perms "$CCMS_HOME/keystore.key")" == "600" ]] && ok "keystore.key 权限 600" || bad "keystore.key 权限不是 600"
expect "aes-256-gcm" "$("$BIN" --vault --status)" "旧式 --vault --status"
expect "可解密      是" "$("$BIN" vault status)" "密钥库可解密"
refute "sk-test-alpha" "$(cat "$CCMS_HOME/keys.enc")" "密文里没有明文密钥"

"$BIN" key kimi remove 1 >/dev/null
COUNT="$("$BIN" key kimi list | grep -c '…' || true)"
[[ "$COUNT" == "1" ]] && ok "remove 生效" || bad "remove 未生效（$COUNT）"
"$BIN" key kimi clear >/dev/null
expect "为空" "$("$BIN" key kimi list)" "clear 生效"

# ---------- stdin 写入（不进入 argv / history） ----------
printf 'sk-pipe-alpha-123456\nsk-pipe-beta-123456\n' | "$BIN" --key kimi --stdin >/dev/null
expect "sk-pip" "$("$BIN" --key kimi list)" "旧式 --stdin 写入并掩码"
refute "sk-pipe-alpha" "$(cat "$CCMS_HOME/keys.enc")" "stdin 写入后密文无明文"
printf 'sk-modern-pipe-123456\n' | "$BIN" key kimi --stdin >/dev/null
expect "sk-mod" "$("$BIN" key kimi list)" "新式 key --stdin"
if printf '  \n' | "$BIN" key kimi --stdin >/dev/null 2>"$TMP/empty-key.err"; then
  bad "空 stdin 应失败"
else
  ok "空 stdin 被拒绝"
fi
expect "stdin 为空" "$(cat "$TMP/empty-key.err")" "空 stdin 提示明确"
"$BIN" key kimi clear >/dev/null

# ---------- 路由（旧式矩阵） ----------
for args in "-r -l" "-r --list" "--route list" "--route --list"; do
  # shellcheck disable=SC2086
  expect "路由表" "$("$BIN" $args)" "旧式 $args"
done
expect "路由表" "$("$BIN" -r)" "缺省 -r = list"
expect "路由表" "$("$BIN" --route)" "缺省 --route = list"

"$BIN" --route --set opus kimi kimi-k3.1-pro >/dev/null
expect "kimi/kimi-k3.1-pro" "$("$BIN" -r -l)" "旧式 --route --set"
"$BIN" -r --unset opus >/dev/null
if "$BIN" -r -l | grep -q 'Opus.*-/-'; then
  ok "旧式 -r --unset"
else
  bad "旧式 -r --unset 未生效"
fi
"$BIN" route set haiku deepseek deepseek-flash >/dev/null
expect "deepseek/deepseek-flash" "$("$BIN" route list)" "新式 route set 不回归"
"$BIN" route all kimi >/dev/null
expect "kimi/kimi-k3.1-pro" "$("$BIN" --route --list)" "旧式 --route --all 后仍可读"

if "$BIN" route set nope kimi x >/dev/null 2>&1; then
  bad "非法档位应被拒绝"
else
  ok "非法档位被拒绝"
fi
if "$BIN" --route --set opus ghost model >/dev/null 2>&1; then
  bad "未知 provider 应被拒绝"
else
  ok "未知 provider 被拒绝"
fi
expect_fail "缺参 --route --set 非零" "$BIN" --route --set
expect "用法: ccms --route/-r --set" "$EXPECT_FAIL_OUT" "缺参 --route --set 旧式用法"
expect_fail "缺参 --key 非零" "$BIN" --key
expect "用法: ccms --key/-k" "$EXPECT_FAIL_OUT" "缺参 --key 旧式用法"

# ---------- 模式切换 + router ----------
# 模拟升级前官方设置：on 必须保存，off 必须恢复；未受管字段全程保留。
printf '%s\n' '{"env":{"ANTHROPIC_API_KEY":"sk-dummy-official","FOO":"keep"},"hooks":{"SessionStart":[]},"permissions":{"allow":["Read"]}}' > "$CLAUDE_SETTINGS"
"$BIN" --on >/dev/null
SETTINGS="$(cat "$CLAUDE_SETTINGS")"
TOKEN="$(cat "$CCMS_HOME/router.token")"
expect "http://127.0.0.1:$PORT" "$SETTINGS" "settings.json 写入 base_url"
expect "ccms-subagent" "$SETTINGS" "settings.json 写入 subagent 档位"
expect "ccms-haiku" "$SETTINGS" "settings.json 写入 haiku 档位"
refute "ccms-router-local" "$SETTINGS" "settings.json 不再写固定占位 token"
refute "sk-dummy-official" "$SETTINGS" "路由期间官方 API key 被受管替换"
expect "$TOKEN" "$SETTINGS" "settings.json 写入随机 router token"
[[ "${#TOKEN}" -ge 43 ]] && ok "router token 长度 >= 43" || bad "router token 过短"
[[ "$(perms "$CCMS_HOME/router.token")" == "600" ]] && ok "router.token 权限 600" || bad "router.token 权限不是 600"
[[ "$(perms "$CCMS_HOME")" == "700" ]] && ok "CCMS_HOME 权限 700" || bad "CCMS_HOME 权限不是 700"
[[ -f "$CCMS_HOME/settings.previous.json" ]] && ok "已保存 settings 前状态" || bad "缺少 settings.previous.json"
[[ "$(perms "$CCMS_HOME/settings.previous.json")" == "600" ]] && ok "settings.previous.json 权限 600" || bad "前状态权限不是 600"
PREVIOUS_BEFORE="$(cat "$CCMS_HOME/settings.previous.json")"
"$BIN" --on >/dev/null
[[ "$(cat "$CCMS_HOME/settings.previous.json")" == "$PREVIOUS_BEFORE" ]] && ok "重复 on 不覆盖前状态" || bad "重复 on 覆盖了前状态"
expect "运行中" "$("$BIN" --router --status)" "旧式 --router --status"
expect "运行中" "$("$BIN" --status)" "旧式 --status"
expect '"router_alive": true' "$("$BIN" status --json)" "新式 status --json 不回归"
PIDFILE="$(cat "$CCMS_HOME/router.pid")"
HEALTH="$(curl -sS --noproxy '*' "http://127.0.0.1:$PORT/ccms/health")"
expect "\"pid\":$PIDFILE" "$HEALTH" "health 报告当前 router pid"
expect '"service":"ccms"' "$HEALTH" "health 标记 service=ccms"
STATUS="$(curl -sS --noproxy '*' -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/v1/models")"
[[ "$STATUS" == "401" ]] && ok "无 token 拒绝 /v1/models" || bad "无 token 应 401（$STATUS）"
STATUS="$(curl -sS --noproxy '*' -o /dev/null -w '%{http_code}' -H 'x-api-key: ccms-router-local' "http://127.0.0.1:$PORT/v1/models")"
[[ "$STATUS" == "401" ]] && ok "固定占位 token 已失效" || bad "固定占位 token 应 401（$STATUS）"
expect "ccms-opus" "$(curl -sS --noproxy '*' -H "x-api-key: $TOKEN" "http://127.0.0.1:$PORT/v1/models")" "/v1/models 返回哨兵（正确 token）"
refute "$TOKEN" "$(cat "$CCMS_HOME/router.log")" "router.log 不泄漏 token"
if "$BIN" --router --logs 5 >/dev/null; then
  ok "旧式 --router --logs"
else
  bad "旧式 --router --logs 失败"
fi

# tail 必须从文件尾有界读取，不能整文件读入。
{
  printf 'tail-first-marker\n'
  awk 'BEGIN { for (i = 0; i < 60000; i++) printf "filler-%06d-%0120d\n", i, i }'
  printf 'tail-last-marker\n'
} >> "$CCMS_HOME/router.log"
TAIL_OUT="$("$BIN" --router --logs 3)"
expect "tail-last-marker" "$TAIL_OUT" "大日志 tail 包含最后标记"
refute "tail-first-marker" "$TAIL_OUT" "大日志 tail 只读末尾"

"$BIN" --router --restart >/dev/null
expect "运行中" "$("$BIN" router status)" "router 重启"
"$BIN" --router --stop >/dev/null
expect "未运行" "$("$BIN" --router --status)" "旧式 --router --stop"

"$BIN" --off >/dev/null
SETTINGS="$(cat "$CLAUDE_SETTINGS")"
refute "ANTHROPIC_BASE_URL" "$SETTINGS" "旧式 --off 清理 env"
expect "sk-dummy-official" "$SETTINGS" "off 恢复官方 API key"
expect "FOO" "$SETTINGS" "off 保留未受管 env"
expect "SessionStart" "$SETTINGS" "off 保留 hooks"
expect "permissions" "$SETTINGS" "off 保留 permissions"
[[ ! -f "$CCMS_HOME/settings.previous.json" ]] && ok "off 成功后删除前状态" || bad "前状态未删除"

# ---------- 诊断 / 统计 / 未知命令 ----------
expect "PROVIDER" "$("$BIN" --list)" "旧式 --list"
expect "环境" "$("$BIN" --doctor)" "旧式 --doctor"
expect "0.2.0" "$("$BIN" --doctor)" "doctor 版本"
expect '"days"' "$("$BIN" --stats --json)" "旧式 --stats --json"
expect "暂无用量记录" "$("$BIN" -u)" "旧式 -u 空态"
expect "暂无用量记录" "$("$BIN" stats)" "新式 stats 不回归"
expect "明文" "$(rm -f "$CCMS_HOME/keys.enc" "$CCMS_HOME/keystore.key"; "$BIN" --vault --status)" "无密钥库时的提示"
CORRUPT_STATS='{"version":2,"daily":'
printf '%s' "$CORRUPT_STATS" > "$CCMS_HOME/stats.json"
expect_fail "损坏 stats 非零" "$BIN" --stats --json
expect "统计" "$EXPECT_FAIL_OUT" "损坏 stats 错误明确"
[[ "$(cat "$CCMS_HOME/stats.json")" == "$CORRUPT_STATS" ]] && ok "损坏 stats 未被覆盖" || bad "损坏 stats 被改写"
expect_fail "未知命令非零" "$BIN" frobnicate
expect "未知命令: frobnicate" "$EXPECT_FAIL_OUT" "未知命令中文提示"
expect "ccms --help/-h 看用法" "$EXPECT_FAIL_OUT" "未知命令提示旧式帮助"
refute "Usage:" "$EXPECT_FAIL_OUT" "未知命令不泄漏 clap Usage"
expect_fail "未知子命令非零" "$BIN" --route frob
expect "未知子命令: frob" "$EXPECT_FAIL_OUT" "未知子命令中文提示"
expect_fail "未知 flag 非零" "$BIN" --bogus
expect "ccms --help/-h 看用法" "$EXPECT_FAIL_OUT" "未知 flag 提示旧式帮助"

printf '\n'
if [[ $FAIL -eq 0 ]]; then
  ok "全部验证通过（隔离目录 $TMP）"
else
  printf '✗ 存在失败项\n' >&2
  exit 1
fi
