#!/usr/bin/env bash
# 在 WSL 中直接验证 Windows 仓库，不复制源码、不影响现有配置和 router。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${CCMS_TEST_PORT:-18788}"
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

# 测试只访问本机；环境里的 http_proxy 会把 127.0.0.1 也代理出去并返回 502。
export no_proxy='*' NO_PROXY='*'

for cmd in node jq curl; do
  command -v "$cmd" >/dev/null || die "缺少 $cmd"
done

bash -n "$ROOT/bin/ccms"
node --check "$ROOT/lib/ccms/router.js"
node --check "$ROOT/lib/ccms/stats.js"
node --check "$ROOT/lib/ccms/keystore.js"
jq -e . "$ROOT/config/routes.json" >/dev/null
jq -e . "$ROOT/config/providers.json" >/dev/null
ok "语法与配置 JSON 校验通过"

VAULT_HOME="$TMP/.config/ccms"
KEYSTORE_JS="$ROOT/lib/ccms/keystore.js"

mkdir -p "$VAULT_HOME" "$TMP/.claude"
jq --argjson port "$PORT" '.listen = ("127.0.0.1:" + ($port|tostring))' \
  "$ROOT/config/routes.json" > "$VAULT_HOME/routes.json"
cp "$ROOT/config/providers.json" "$VAULT_HOME/providers.json"
: > "$VAULT_HOME/keys.env"
printf '{}\n' > "$TMP/.claude/settings.json"

run_ccms() {
  HOME="$TMP" \
  CCMS_HOME="$VAULT_HOME" \
  CCMS_ROUTER_JS="$ROOT/lib/ccms/router.js" \
  CCMS_STATS_JS="$ROOT/lib/ccms/stats.js" \
  CCMS_KEYSTORE_JS="$KEYSTORE_JS" \
  CLAUDE_SETTINGS="$TMP/.claude/settings.json" \
  NO_COLOR=1 bash "$ROOT/bin/ccms" "$@"
}

[[ "$(run_ccms --version)" == *"0.1.0"* ]] || die "ccms --version 输出异常"
run_ccms --route --list >/dev/null
run_ccms >/dev/null
run_ccms --help >/dev/null
ok "CLI 冒烟测试通过"

# ---------- 加密密钥库 ----------
# 预置明文，验证首次 --key 会自动把它迁移进加密库并抹掉明文。
printf 'KIMI_API_KEYS=sk-legacy-plain\n' > "$VAULT_HOME/keys.env"
run_ccms --key kimi sk-test-alpha >/dev/null

[[ -f "$VAULT_HOME/keys.enc" ]] || die "未生成 keys.enc"
[[ -f "$VAULT_HOME/keystore.key" ]] || die "未生成 keystore.key"
[[ "$(stat -c '%a' "$VAULT_HOME/keystore.key")" == "600" ]] \
  || die "keystore.key 权限应为 600"
[[ "$(stat -c '%a' "$VAULT_HOME/keys.enc")" == "600" ]] \
  || die "keys.enc 权限应为 600"
jq -e '.cipher == "aes-256-gcm" and .version == 1' "$VAULT_HOME/keys.enc" >/dev/null \
  || die "keys.enc 元数据异常"
if grep -q 'sk-legacy-plain\|sk-test-alpha' "$VAULT_HOME/keys.env"; then
  die "明文仍残留在 keys.env"
fi
ok "加密库创建、权限与明文清除通过"

plain="$(CCMS_HOME="$VAULT_HOME" node "$KEYSTORE_JS" open)"
[[ "$plain" == *"sk-test-alpha"* ]] || die "加密库解密失败"
[[ "$plain" == *"sk-legacy-plain"* ]] || die "旧明文未随首次写入迁移"
[[ "$(CCMS_HOME="$VAULT_HOME" node "$KEYSTORE_JS" get KIMI_API_KEYS)" == "sk-legacy-plain,sk-test-alpha" ]] \
  || die "迁移后的密钥池内容异常"
listed="$(run_ccms --key kimi list)"
[[ "$listed" == *"sk-le"* ]] || die "--key list 未显示掩码密钥"
run_ccms --key deepseek sk-test-beta >/dev/null
[[ "$(CCMS_HOME="$VAULT_HOME" node "$KEYSTORE_JS" get DEEPSEEK_API_KEYS)" == "sk-test-beta" ]] \
  || die "写入第二家 provider 失败"
vstatus="$(run_ccms --vault --status)"
[[ "$vstatus" == *"aes-256-gcm"* ]] || die "--vault --status 异常"
ok "加解密往返、明文迁移、CLI 读取通过"

# 单引号、$ 等字符不得破坏 bash 的 source 路径。
printf '%s' "sk-'quote'-\$dollar" | CCMS_HOME="$VAULT_HOME" node "$KEYSTORE_JS" set QWEN_API_KEYS
[[ "$(CCMS_HOME="$VAULT_HOME" node "$KEYSTORE_JS" get QWEN_API_KEYS)" == "sk-'quote'-\$dollar" ]] \
  || die "特殊字符往返失败"
run_ccms --list >/dev/null || die "source 加密库输出失败"
ok "特殊字符与 shell source 路径通过"

# 口令模式：主密钥不落盘，错误口令/缺口令必须解密失败。
PASSLIB="$TMP/passlib"
mkdir -p "$PASSLIB"
printf '%s' 'sk-passphrase' | CCMS_HOME="$PASSLIB" CCMS_KEYSTORE_PASSPHRASE='correct horse' \
  node "$KEYSTORE_JS" set KIMI_API_KEYS
[[ -f "$PASSLIB/keys.enc" ]] || die "口令模式未生成密钥库"
[[ ! -e "$PASSLIB/keystore.key" ]] || die "口令模式不应生成主密钥文件"
[[ "$(CCMS_HOME="$PASSLIB" CCMS_KEYSTORE_PASSPHRASE='correct horse' node "$KEYSTORE_JS" get KIMI_API_KEYS)" == "sk-passphrase" ]] \
  || die "口令模式解密失败"
if CCMS_HOME="$PASSLIB" CCMS_KEYSTORE_PASSPHRASE='wrong' \
   node "$KEYSTORE_JS" open >/dev/null 2>&1; then
  die "错误口令竟然解密成功"
fi
if CCMS_HOME="$PASSLIB" node "$KEYSTORE_JS" open >/dev/null 2>&1; then
  die "缺口令竟然解密成功"
fi
ok "口令模式与失败关闭通过"

# 轮换主密钥后数据不丢，且 router 仍能读取。
CCMS_HOME="$VAULT_HOME" node "$KEYSTORE_JS" rekey >/dev/null 2>&1 \
  || die "rekey 失败"
[[ "$(CCMS_HOME="$VAULT_HOME" node "$KEYSTORE_JS" get KIMI_API_KEYS)" == "sk-legacy-plain,sk-test-alpha" ]] \
  || die "rekey 后数据丢失"
ok "主密钥轮换通过"

# ---------- router ----------
HOME="$TMP" CCMS_HOME="$VAULT_HOME" node "$ROOT/lib/ccms/router.js" >"$TMP/router.log" 2>&1 &
PID=$!

for _ in $(seq 1 30); do
  sleep 0.1
  if curl -fsS -m 1 "http://127.0.0.1:$PORT/ccms/health" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$PID" 2>/dev/null || { cat "$TMP/router.log"; die "router 启动失败"; }
done

health="$(curl -fsS -m 2 "http://127.0.0.1:$PORT/ccms/health")"
jq -e '.ok == true' <<<"$health" >/dev/null || die "router 健康检查失败"
jq -e '.keys.kimi == 2 and .keys.deepseek == 1 and .keys.qwen == 1' <<<"$health" >/dev/null \
  || die "router 未能读取加密密钥库"
run_ccms --router --status >/dev/null
ok "router 隔离启动、健康检查与加密库读取通过（127.0.0.1:$PORT）"

kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
PID=""
ok "全部验证通过"
