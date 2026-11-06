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

需要 Rust 1.82+（`https://rustup.rs`，文件锁使用 `fs2`，不依赖 1.89 才稳定的标准库锁 API）和平台常规 C 链接器。

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
ccms --init                               # 生成默认配置（首次运行时也会自动生成）
ccms --key deepseek sk-xxxx               # 追加一把密钥（更推荐用 --stdin 避免进 shell history）
printf 'sk-aaaa\nsk-bbbb\n' | ccms --key kimi --stdin
ccms --route --set sonnet qwen qwen3-coder-plus
ccms --test deepseek                      # 验证单家链路
ccms --on                                 # 启动 router + 改写 Claude settings.json
ccms --test router                        # 验证整条路由
claude                                    # 新开会话生效
```

## 命令

帮助与首页保持 0.1 的旧式风格：裸 `ccms` 是首页，`ccms --help`（或 `ccms -h`）是完整的长 flag
帮助页（不会出现 clap 的 `Usage:` / `Commands:` 页面）。现代子命令仍可用。

| 目的 | 旧式写法（主） | 0.2 子命令（兼容） |
|---|---|---|
| 首页（模式 / 默认档位 / 路由 / 密钥） | `ccms` | `ccms` |
| 帮助 / 版本 | `ccms --help`、`ccms --version` | `ccms version` |
| 初始化配置 | `ccms --init [--force]` | `ccms init [--force]` |
| 启用 / 关闭路由 | `ccms --on`、`ccms --off` | `ccms on`、`ccms off` |
| 详细状态（可 `--json`） | `ccms --status [--json]` | `ccms status [--json]` |
| provider 列表 | `ccms --list` | `ccms list` |
| 密钥池 | `ccms --key <p> <key>`、`list`、`remove <n>`、`clear` | `ccms key ...` |
| 加密密钥库 | `ccms --vault --status`、`--seal`、`--open`、`--rekey` | `ccms vault ...` |
| 端点模型列表 | `ccms --models <p>` | `ccms models <p>` |
| 链路测试 | `ccms --test <p\|router\|all>` | `ccms test ...` |
| 路由表 | `ccms --route --list`、`--set`、`--unset`、`--all` | `ccms route ...` |
| 路由器进程 | `ccms --router --start`、`--stop`、`--restart`、`--status`、`--logs [n]` | `ccms router ...` |
| 用量统计 | `ccms --stats [天数] [--by ...] [--json]`、`ccms --stats reset` | `ccms stats ...` |
| 环境自检 | `ccms --doctor` | `ccms doctor` |

短名与长名完全等价：`-k`/`--key`、`-m`/`--models`、`-t`/`--test`、`-d`/`--doctor`、
`-u`/`--stats`、`-r`/`--route`，例如 `ccms -r -l` 是可直接执行的命令。
嵌套解析与 0.1 一致：裸词、`--长名`、既有短名都接受，缺省
`route=list`、`router=status`、`vault=status`。例如 `ccms -r -l`、
`ccms --route --set opus kimi kimi-k3`、`ccms --router --logs 5` 都可直接使用。
未知命令 / 子命令与缺参以非零退出，并给出一行中文用法提示。

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
- 两个文件都按 mtime 热加载，`ccms --route --set` 后立刻生效。

## 模式切换与回滚

`ccms --on` 首次从非路由状态启用时，会把受管 env 键和顶层 `model` 的原值（或“原本不存在”）
写入 `$CCMS_HOME/settings.previous.json`（权限 600）。重复执行 `ccms --on` 不会覆盖这份前状态；
`ccms --off` 移除 ccms 受管值并恢复原值，成功写回后才删除前状态文件。路由期间新增的
未受管 env、`hooks`、`permissions` 等修改都会保留。没有前状态文件的旧升级用户只会删除
ccms 受管值，不会虚构官方 key。`env` 不是对象或备份失败时命令会报错，原文件字节不变。

每次 `on` / `off` 改写 `settings.json` 前，原文件都会备份为 `<settings>.ccms.bak`（权限 600），
同一固定路径只保留最近一次，不会累积历史版本。这份备份在 `off` 之后仍然保留，不会自动删除；如果原 `settings.json` 含官方明文凭据，
`.ccms.bak` 就是同等级的敏感副本，请按原文件的标准保管或自行清理，避免留下被忽略的隐藏副本。

`ccms --test router` 对本机 router 的探测请求使用 60s 超时（与手工 `curl -m60` 一致），
因为真实上游可能明显慢于健康检查；router 自身的 `/ccms/health` 探活仍保持 1500ms。
两类本机请求都强制 `no_proxy`，不会继承 `CCMS_PROXY`。

## 本地安全边界

- `CCMS_HOME`（默认 `~/.config/ccms`）权限收紧到 700；`router.token` 是每个环境随机生成的
  base64url token，文件权限 600，已存在的旧文件也会被收紧。首次创建由同为 600 的
  `router.token.lock` 跨进程串行化，其他进程不会读到空文件或半写入 token。没有
  `HOME` / `USERPROFILE` 且没有 `CCMS_HOME` / `CLAUDE_SETTINGS` 显式覆盖时直接报错，
  绝不把密钥写进当前目录。
- `/v1/models` 与代理端点都要求 `x-api-key` 或 `Authorization: Bearer <router.token>`；
  Claude Code 设置里不再使用固定占位 token。
- router 只监听回环地址，健康响应带 `service=ccms` 与当前 `pid`；停止进程前必须同时核对
  health pid 和 `router.pid`，缺失或不一致时拒绝发送信号。pidfile 只接受正数且不超出
  `i32::MAX`；Unix 发送前用 `i32::try_from` 转原生 PID（避免 PID 0/负数被解释成进程组），
  Windows 只用 `taskkill /PID <pid> /F`，不加 `/T`，不会连带杀无关进程树；kill / taskkill
  失败会作为命令错误返回。
- `router.log` 在运行期按 5 MiB 轮转（保留 `router.log.1`），轮转/打开/写入失败会写到
  stderr；`ccms --router --logs` 从文件尾按固定块反向读取，缓冲最多 1 MiB 并做 lossy UTF-8。
- 转发前清洗逐跳头（含 `Connection` 里列出的字段）、`cookie`、代理凭据和本地鉴权头。
- 非流式上游响应以实际读取字节计数限制为 16 MiB，不能只信 `Content-Length`；SSE 客户端断开
  会立即取消上游读取，不必等下一个 chunk。

## 密钥库

真实密钥只存在 `keys.enc`（AES-256-GCM，权限 600），`keys.enc` 里是可读元数据，
`data` 是密文；`iv` 每次写入重新随机，GCM tag 校验完整性，AAD 绑定版本与模式。

| 模式 | 主密钥 | 启用方式 |
|---|---|---|
| `keyfile`（默认） | `~/.config/ccms/keystore.key`（32 字节随机） | 首次写入自动生成 |
| `passphrase` | `CCMS_KEYSTORE_PASSPHRASE` 经 scrypt 派生，不落盘 | 首次写入前 export 该变量 |

```bash
ccms --vault --status     # 加密状态 / 主密钥来源 / 条目数
ccms --vault --seal       # 把明文 keys.env 迁移进 keys.enc
ccms --vault --rekey      # 轮换主密钥（journal 事务，崩溃可恢复）
ccms --vault --open       # 输出明文，谨慎
```

- 首次写密钥时若存在明文 `keys.env`，会自动迁移并把明文文件清空为注释。
- 密钥库模式一旦建立就锁定，只有 `vault rekey` 才能换模式。
- `keyfile` 模式下 `keys.enc` 与 `keystore.key` 必须一起备份，缺一不可解。
- 兼容 0.1（Node 版）生成的 `keys.enc` / `keystore.key`，可直接升级。
- 密钥读取按 `keys.enc` / `keystore.key` 的 mtime 缓存；解密失败时不会记录缓存戳，
  `loaded` 保持为 false，下一次请求会重新尝试。失败的那次请求以空 key 池 fail closed（401），
  不需要重启 router 或修改配置。
- `status` 里的 passphrase “不落盘”只描述**当前库的主密钥来源**：当前版本不会读取旧的
  `keystore.key`，也不会自动删除它。如果目录里仍保留该文件，它可能是旧版本 / 旧 keyfile
  模式的备份；不要因为看到“不落盘”就认为历史上从未存在过该文件。

### 事务、锁与错误语义

- 所有修改（`key` 增删、`vault seal/rekey`、首次迁移）都在同一
  `$CCMS_HOME/keystore.lock` 排他锁内完成整个 load-modify-persist；锁文件权限 600。
  多个进程同时写不会互相覆盖，reader 仍然只依赖 `rename` 原子替换，不需要全局读锁。
- `rekey` 先把旧 `keys.enc` / `keystore.key` 备份为固定文件名
  （`keys.enc.rekey.bak`、`keystore.key.rekey.bak`），再写
  `keystore.rekey.journal`（状态依次为 `prepared` → `enc-written` → `committed`）。
  任何未到 `committed` 的中断，都会在下一次 load/status/写入/迁移时用备份恢复旧密钥对；
  已 `committed` 则保留新密钥对并清理事务文件。所有中间文件权限均为 600。
- 明文 `keys.env` 的读取错误（包括非 UTF-8）会直接报错，绝不会被当作空库；
  只有文件不存在才表示“没有明文可迁移”。因此 key 更新/seal/migrate 在明文损坏时
  返回错误，且不会创建只含新值的 `keys.enc`。

## 用量统计

按**本地日期**分桶，保留 180 天，写入是临时文件 + rename。每次请求记录：

```
requests / input_tokens / output_tokens / cache_read / cache_creation
retries / errors / duration_ms
```

维度：`total`、`providers`、`tiers`、`models`（`provider/model`）。流式响应边转发边解析
`message_start` 与 `message_delta`，不缓冲整段回答。

```bash
ccms --stats                # 今日 + 最近 7 天 + 三个维度
ccms --stats 30 --by model  # 只看模型维度
ccms --stats --json
ccms --stats reset
```

统计写入通过 `spawn_blocking` 执行，不阻塞异步转发线程。若 `stats.json` 损坏或不可读，
`stats` 命令会报错，router 只记录错误并保留原文件，绝不会拿一条新记录覆盖损坏数据。

## 架构

```
src/main.rs           命令分发
src/cli.rs            argv 适配层（共享别名表）+ 单一 clap dispatch 树 + 旧式帮助
src/config.rs         routes / providers 模型与默认值（内嵌进二进制）
src/keystore.rs       AES-256-GCM 密钥库
src/router/mod.rs     代理、重试、热加载、健康检查
src/router/sse.rs     流式 usage 增量解析
src/stats.rs          统计写入与聚合
src/settings.rs       Claude settings.json 的 env 管理
src/proc.rs           守护进程启动 / 停止 / 日志
src/commands/*.rs     各子命令
tests/e2e.rs          真实二进制 + mock 上游的端到端测试
tests/cli_contract.rs 真实二进制的 CLI 契约测试（首页 / 帮助 / 旧式嵌套 / home 缺失）
tests/static_contract.rs 源码静态契约（install.ps1 / MSRV / fs2 / Windows 边界）
```

当前测试：WSL/Linux `cargo test --all-targets` 为 82 单元 + 15 CLI 契约 + 2 端到端 +
5 静态契约，共 104 项通过（另有 1 个多进程测试子进程入口按设计 ignored）；Windows 为
72 单元 + 15 + 2 + 5，共 94 项通过、同样 1 个 helper ignored，Unix 权限测试按平台跳过。
`./scripts/validate.sh` 用临时 `CCMS_HOME` 跑旧式命令矩阵和大日志 tail，不联网。

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
  两者都未显式设置时必须有 `HOME` / `USERPROFILE`，否则启动报错而不是回退到 `./`。
- 守护进程：Unix 用 `setsid` + SIGTERM，Windows 用 `DETACHED_PROCESS` + `taskkill`。
- 默认**不读**系统 `http_proxy`（避免劫持 127.0.0.1），需要代理时显式设置 `CCMS_PROXY`。
- 只监听回环地址；启动时会拒绝非 `127.0.0.1` 的 `listen`。
- 流式响应一旦开始转发就不再重连，避免重复输出。
- 不计算费用，只统计 token 与请求数。

## 注意

- 路由模式下 Claude Code 使用随机 router token，真实密钥只存在加密库中。
- 修改 `settings.json` 的 env 段请用 `ccms on/off`，不要手改。
- 第三方端点的计费与额度由对应服务决定；工具调用、缓存、会话压缩的表现可能与官方不同。
