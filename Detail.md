# Alkanet 技术说明

## 1. 架构与分工

Alkanet 1.0.0 采用 **Go CLI (`aknt`) + Rust 守护进程 (`alkanetd`)** 双二进制架构：

- **`aknt`（Go + Cobra）**：负责命令行交互、看板与状态展示、TOML 配置管理（`config.toml`）、Claude `settings.json` 接管与恢复、守护进程启停控制与统计聚合展示。
- **`alkanetd`（Rust + Tokio/Axum）**：负责高性能 HTTP 代理、SSE 流式 usage 解析、多 key 轮询与退避重试、`stats.json` 写入，以及 AES-256-GCM 加密密钥库与 `fs2` 跨进程事务锁（通过内部 `alkanetd vault ...` 命令供 `aknt` 调用）。

```
Claude Code (settings.json env)
  │ ANTHROPIC_BASE_URL=http://127.0.0.1:8788
  │ ANTHROPIC_MODEL=alkanet-sonnet 等
  ▼
alkanetd run
  ├─ 校验本机 router token
  ├─ resolve_tier(model)：精确哨兵 (alkanet-/ccms-) → 关键字 → 默认档位
  ├─ config.toml[routes.tier] → provider + model（mtime 热加载）
  ├─ 解密 keys.enc，按 provider 游标取一把 key
  ├─ 改写 body.model，替换鉴权头
  ├─ 上游转发（流式 / 非流式）
  ├─ 响应头前失败 → 换 key / 退避重试
  └─ 解析 usage → stats.json
```

守护进程提供 `/alkanet/health`（兼容 `/ccms/health`）与 `/v1/models`。

## 2. 配置（`config.toml`）

运行期配置默认位于 `$ALKANET_HOME/config.toml`（默认 `~/.config/alkanet/config.toml`；环境变量仍兼容 `CCMS_HOME`）。

- `listen`：默认 `127.0.0.1:8788`，仅允许回环地址。
- `default` / `effort`：默认档位（`sonnet`）与 `CLAUDE_CODE_EFFORT_LEVEL`（`high`）。
- `[retry]`：`attempts`、`backoff_ms`、`response_timeout_ms`、`connect_timeout_ms`、`retry_on`。
- `[routes.<tier>]`：`fable / opus / sonnet / haiku / subagent` 五档绑定 `provider` 与 `model`。
- `[providers.<name>]`：`label`、`base_url`、`key_env`、`default_model`、`models`、`auth`（`bearer` 或 `x-api-key`）、`note`。

`alkanetd` 按 `config.toml` 的 `(mtime, len)` 自动热加载；若 `config.toml` 不存在且存在旧版 `routes.json` + `providers.json`，则自动回退读取旧版 JSON 配置。

## 3. 模式切换与 settings 回滚

`aknt on` 首次从非路由状态启用时，将受管环境变量与顶层 `model` 的原值快照到 `$ALKANET_HOME/settings.previous.json`（权限 600），每次改写前备份为 `<settings>.alkanet.bak`（权限 600）。`aknt off` 移除受管值、恢复前状态并删除 `settings.previous.json`，保留路由期间新增的未受管配置。

## 4. 密钥库与安全边界

- `keys.enc` 采用 AES-256-GCM 加密，保留对 0.1/0.2 密钥库格式与 `ccms-keystore:<version>:<mode>` AAD 的完全兼容；支持 `keyfile`（`keystore.key`）与 `passphrase`（`ALKANET_KEYSTORE_PASSPHRASE` / `CCMS_KEYSTORE_PASSPHRASE`，scrypt N=16384, r=8, p=1）两种模式。
- 所有写事务与 `rekey` 均持有 `keystore.lock` 排他锁并带 journal 恢复机制。
- `router.token` 为每个配置目录随机生成的 32 字节 base64url token（权限 600）。
- 默认不继承系统 `http_proxy`；需要上游代理时显式设置 `ALKANET_PROXY`（兼容 `CCMS_PROXY`）。

## 5. 平台验证范围

- Linux / WSL：完整运行 `cargo fmt`、`cargo clippy`、`cargo test`、`go vet`、`go test`、release 构建及 `./scripts/validate.sh` 隔离验证。
- Windows 原生：已验证 `alkanetd` clippy / 单元测试 / 端到端测试与 `aknt.exe` 交叉编译。
- macOS：走 Unix 分支，未在真机编译（仅代码审查）。
