# CCMS 0.2 — Claude Code Model Switcher

用 Rust 重写的 Claude Code 前置路由器：**每个档位可以独立选择供应商和模型**，
自带加密密钥池与按模型维度的用量统计。单个二进制，Windows / macOS / Linux 通用。

```
Claude Code ──► 127.0.0.1:8788 ──► 按档位分发 ──► 不同供应商的 Anthropic 兼容端点
   fable   → kimi/kimi-k3.1-pro
   opus    → kimi/kimi-k3.1-pro
   sonnet  → qwen/qwen3-coder-plus
   haiku   → deepseek/deepseek-flash
   subagent→ deepseek/deepseek-flash
```

## 特性

- **档位路由**：`fable / opus / sonnet / haiku / subagent` 五个档位各自绑定 `<provider>/<model>`，
  Claude Code 只看到哨兵名 `ccms-<tier>`。
- **密钥池**：每家 provider 可挂多把 key，按请求轮询，失败自动换 key 重试。
- **加密存储**：AES-256-GCM（`keys.enc`），主密钥与密文分离；支持 keyfile 与 passphrase 两种模式。
- **用量统计**：按模型 / 供应商 / 档位聚合请求数、token、缓存 token、重试、错误数、平均耗时。
- **热加载**：改完路由或密钥即时生效，不需要重启 router。
- **可迁移**：单个可执行文件；配置在 `~/.config/ccms`，三平台路径一致。

## 安装

需要 Rust 1.82+（`https://rustup.rs`）和平台常规 C 链接器。

**Linux / macOS / WSL**

```bash
git clone https://github.com/Madokana127/ccms.git
cd ccms
./install.sh                 # 构建 → 安装到 ~/.local/bin → 初始化配置
./scripts/validate.sh        # 隔离验证（不需要真实密钥、不联网）
```

**Windows（PowerShell）**

```powershell
git clone https://github.com/Madokana127/ccms.git
cd ccms
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

安装脚本只做四件事：构建 release、把二进制放到用户目录、写入 PATH、`ccms init`。
已有 `routes.json` / `providers.json` / `keys.enc` 一律不覆盖。卸载用 `--uninstall`（配置保留）。

## 快速开始

```bash
ccms init                              # 生成默认配置（首次运行时也会自动生成）
ccms key deepseek sk-xxxx              # 追加一把密钥
ccms key kimi sk-aaaa,sk-bbbb          # 一次加多把，轮询使用
ccms route set sonnet qwen qwen3-coder-plus
ccms test deepseek                     # 验证单家链路
ccms on                                # 启动 router + 改写 Claude settings.json
ccms test router                       # 验证整条路由
claude                                 # 新开会话生效
```

## 命令

| 目的 | 命令 |
|---|---|
| 首页（模式 / 默认档位 / 路由 / 密钥） | `ccms` |
| 初始化配置 | `ccms init [--force]` |
| 启用 / 关闭路由 | `ccms on`、`ccms off` |
| 详细状态（可 `--json`） | `ccms status` |
| provider 列表 | `ccms list` |
| 密钥池 | `ccms key <p> <key>`、`list`、`remove <n>`、`clear` |
| 加密密钥库 | `ccms vault [status|seal|open|rekey]` |
| 端点模型列表 | `ccms models <p>` |
| 链路测试 | `ccms test <p|router|all>` |
| 路由表 | `ccms route [list]`、`set <tier> <p> <model>`、`unset <tier>`、`all <p> [model]` |
| 路由器进程 | `ccms router [start|stop|restart|status|logs [n]]` |
| 用量统计 | `ccms stats [天数]`、`--by model|provider|tier`、`--json`、`reset` |
| 环境自检 | `ccms doctor` |

0.1 的写法（`ccms --on`、`--key`、`--route --list`…）会在解析前被翻译成子命令，
老脚本和肌肉记忆都不作废。`ccms --help` 看完整说明。

## 配置

`~/.config/ccms/routes.json`：监听地址、默认档位、effort、重试参数、五档路由。

```json
{
  "listen": "127.0.0.1:8788",
  "default": "sonnet",
  "effort": "high",
  "retry": {
    "attempts": 3,
    "backoff_ms": 400,
    "response_timeout_ms": 120000,
    "connect_timeout_ms": 15000,
    "retry_on": [401, 403, 408, 429, 500, 502, 503, 504]
  },
  "routes": { "sonnet": { "provider": "qwen", "model": "qwen3-coder-plus" } }
}
```

`~/.config/ccms/providers.json`：供应商事实，不再重复保存每个档位的模型。

```json
{
  "kimi": {
    "label": "Kimi (Moonshot)",
    "base_url": "https://api.moonshot.cn/anthropic",
    "key_env": "KIMI_API_KEY",
    "default_model": "kimi-k3.1-pro",
    "models": ["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed"],
    "auth": "bearer"
  }
}
```

- `key_env` 推导池变量：`*_KEY` → `*_KEYS`，也可用 `key_env_pool` 显式指定。
- `auth` 可选 `bearer`（默认）或 `x-api-key`，适配只认 `x-api-key` 的网关。
- 两个文件都按 mtime 热加载，`ccms route set` 后立刻生效。

## 密钥库

真实密钥只存在 `keys.enc`（AES-256-GCM，权限 600），`keys.enc` 里是可读元数据，
`data` 是密文；`iv` 每次写入重新随机，GCM tag 校验完整性，AAD 绑定版本与模式。

| 模式 | 主密钥 | 启用方式 |
|---|---|---|
| `keyfile`（默认） | `~/.config/ccms/keystore.key`（32 字节随机） | 首次写入自动生成 |
| `passphrase` | `CCMS_KEYSTORE_PASSPHRASE` 经 scrypt 派生，不落盘 | 首次写入前 export 该变量 |

```bash
ccms vault status     # 加密状态 / 主密钥来源 / 条目数
ccms vault seal       # 把明文 keys.env 迁移进 keys.enc
ccms vault rekey      # 轮换主密钥（失败自动回滚）
ccms vault open       # 输出明文，谨慎
```

- 首次写密钥时若存在明文 `keys.env`，会自动迁移并把明文文件清空为注释。
- 密钥库模式一旦建立就锁定，只有 `vault rekey` 才能换模式。
- `keyfile` 模式下 `keys.enc` 与 `keystore.key` 必须一起备份，缺一不可解。
- 兼容 0.1（Node 版）生成的 `keys.enc` / `keystore.key`，可直接升级。
- 密钥读取按文件 mtime 缓存，口令模式不会每次请求都跑 scrypt。

## 用量统计

按**本地日期**分桶，保留 180 天，写入是临时文件 + rename。每次请求记录：

```
requests / input_tokens / output_tokens / cache_read / cache_creation
retries / errors / duration_ms
```

维度：`total`、`providers`、`tiers`、`models`（`provider/model`）。流式响应边转发边解析
`message_start` 与 `message_delta`，不缓冲整段回答。

```bash
ccms stats                 # 今日 + 最近 7 天 + 三个维度
ccms stats 30 --by model   # 只看模型维度
ccms stats --json
ccms stats reset
```

## 架构

```
src/main.rs           命令分发
src/cli.rs            clap 定义 + 旧参数翻译
src/config.rs         routes / providers 模型与默认值（内嵌进二进制）
src/keystore.rs       AES-256-GCM 密钥库
src/router/mod.rs     代理、重试、热加载、健康检查
src/router/sse.rs     流式 usage 增量解析
src/stats.rs          统计写入与聚合
src/settings.rs       Claude settings.json 的 env 管理
src/proc.rs           守护进程启动 / 停止 / 日志
src/commands/*.rs     各子命令
tests/e2e.rs          真实二进制 + mock 上游的端到端测试
```

请求流程：

```
ANTHROPIC_BASE_URL=http://127.0.0.1:8788  ANTHROPIC_MODEL=ccms-sonnet
  → 解析档位（哨兵名 / 模型名关键字 / 默认档位）
  → routes.json 查 provider + model
  → keys.enc 解密，按游标取一把 key
  → 改写 body.model，替换鉴权头
  → 转发；响应头前失败才换 key 重试
  → 解析 usage，写入 stats.json
```

## 从 0.1 迁移

- 配置文件字段兼容：旧的 `providers.json`（`model` + 每档位模型字段）仍可解析，
  `routes.json` 的 `timeout_ms` 会被当作 `response_timeout_ms`。
- 密钥库格式完全一致，无需重新录入密钥。
- 旧的 `--flag` 写法保留；`state.json` 不再使用（模式直接看 `settings.json`）。
- 统计文件新增 `models` / `errors` / `duration_ms` 字段，旧文件照常读取。

## 平台说明

- 配置目录三平台统一为 `$CCMS_HOME` 或 `~/.config/ccms`；Claude 设置可用 `CLAUDE_SETTINGS` 覆盖。
- 守护进程：Unix 用 `setsid` + SIGTERM，Windows 用 `DETACHED_PROCESS` + `taskkill`。
- 默认**不读**系统 `http_proxy`（避免劫持 127.0.0.1），需要代理时显式设置 `CCMS_PROXY`。
- 只监听回环地址；启动时会拒绝非 `127.0.0.1` 的 `listen`。
- 流式响应一旦开始转发就不再重连，避免重复输出。
- 不计算费用，只统计 token 与请求数。

## 注意

- 路由模式下 Claude Code 使用占位 token，真实密钥只存在加密库中。
- 修改 `settings.json` 的 env 段请用 `ccms on/off`，不要手改。
- 第三方端点的计费与额度由对应服务决定；工具调用、缓存、会话压缩的表现可能与官方不同。
