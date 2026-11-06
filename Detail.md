# CCMS 技术说明

## 1. 架构与请求流程

CCMS 是单进程、单端口的本地路由器。Claude Code 只看到哨兵模型名 `ccms-<tier>`，
真实供应商与模型由 `routes.json` 决定。

```
Claude Code
  │ ANTHROPIC_BASE_URL=http://127.0.0.1:8788
  │ ANTHROPIC_MODEL=ccms-sonnet 等
  ▼
ccms --router --run（隐藏入口）
  ├─ 校验本机 router token
  ├─ resolve_tier(model)：精确哨兵 → 模型名关键字 → 默认档位
  ├─ routes.json[tier] → provider + model
  ├─ 解密 keys.enc，按 provider 游标取一把 key
  ├─ 改写 body.model，替换鉴权头
  ├─ 上游转发（流式 / 非流式）
  ├─ 响应头前失败 → 换 key / 退避重试
  └─ 解析 usage → stats.json
```

档位固定为 `fable / opus / sonnet / haiku / subagent`。五个档位可指向不同供应商，
互不影响。router 同时提供 `/ccms/health` 与 `/v1/models`。

## 2. 配置

`routes.json`：

| 字段 | 默认 / 说明 |
|---|---|
| `listen` | `127.0.0.1:8788`；启动时强制校验为回环地址 |
| `default` | `sonnet`；未匹配到档位时使用 |
| `effort` | `high`；写入 `CLAUDE_CODE_EFFORT_LEVEL` |
| `retry.attempts` | 总尝试次数，默认 3 |
| `retry.backoff_ms` | 重试基础退避，默认 400 |
| `retry.response_timeout_ms` | 等待响应头超时；旧名 `timeout_ms` 仍接受 |
| `retry.connect_timeout_ms` | 连接超时 |
| `retry.retry_on` | 触发重试的状态码列表 |
| `routes` | `tier → { provider, model }` |

`providers.json`：

| 字段 | 说明 |
|---|---|
| `label` | 展示名称 |
| `base_url` | Anthropic 兼容端点 |
| `key_env` | 单 key 环境变量名 |
| `key_env_pool` | 可选；显式指定池变量，默认由 `key_env` 推导（`*_KEY` → `*_KEYS`） |
| `default_model` | 默认模型 |
| `models` | 已知模型列表 |
| `auth` | `bearer`（默认）或 `x-api-key` |
| `note` | 备注，可省略 |

两个文件都按 mtime 热加载，修改后立即生效，无需重启。内置默认配置编译进二进制；
`--init` 只在文件缺失时写入，`--init --force` 才重置。

## 3. 模式切换与 settings 回滚

`--on` 首次从非路由状态启用时：

1. 把 CCMS 受管 env 键与顶层 `model` 的原值（包括“原本不存在”）写入
   `$CCMS_HOME/settings.previous.json`，权限 600。
2. 写入 router 地址、随机 token、`ccms-<tier>` 模型名与 effort。
3. 重复执行 `--on` 不覆盖已保存的前状态。

`--off` 移除受管值、恢复前状态，写回成功后才删除 `settings.previous.json`。
路由期间新增的未受管 env、`hooks`、`permissions` 等修改都会保留。没有前状态文件的
旧升级用户只会删除 CCMS 受管值，不会虚构官方 key。

每次 `on` / `off` 改写前，原文件备份为 `<settings>.ccms.bak`（600），固定单路径，
只保留最近一次，`off` 后仍保留。若原文件含明文凭据，该备份同样敏感。
`env` 不是对象、备份失败或写入失败时命令报错，原文件字节不变。
`ccms-` 与 `ccx-` 模型标记只有在本地 `base_url` 与 `ANTHROPIC_AUTH_TOKEN` 同时存在时
才判定为路由器状态，不会被当作官方原值快照。

## 4. 密钥库

`keys.enc` 是可读 JSON 元数据（`version/cipher/mode/iv/tag/data/updated`）加
AES-256-GCM 密文。AAD 绑定 `ccms-keystore:<version>:<mode>`，篡改元数据会导致解密失败；
`iv` 每次写入重新随机，GCM tag 校验完整性。

| 模式 | 主密钥 |
|---|---|
| `keyfile`（默认） | `keystore.key`，32 字节随机，首次写入自动生成 |
| `passphrase` | `CCMS_KEYSTORE_PASSPHRASE` 经 scrypt（N=16384, r=8, p=1）派生，不落盘 |

- 模式一旦建立即锁定，只有 `--vault --rekey` 能切换。
- 所有写入（key 增删、seal、rekey、首次迁移）都在 `keystore.lock` 排他锁内完成
  完整 load-modify-persist；锁内重新加载，避免并发覆盖。
- 读取不需要锁，依赖 `keys.enc` 的原子 rename。
- `rekey` 是 journal 事务：备份旧密文 / 旧 keyfile → 写 `prepared` → 写新密文 →
  `enc-written` → 写新 keyfile → `committed` → 清理。未到 `committed` 的中断会在
  下一次读取 / 状态 / 写入时用备份恢复；`committed` 则保留新密钥对。
- 首次写入时若存在明文 `keys.env`，会自动迁移并把明文文件清空为注释。
  非 UTF-8 或其他读取错误直接报错，只有文件不存在才表示“没有明文”。
- 解密失败时 `loaded` 保持 false，本次请求以空 key 池 fail closed（401）；
  下一次请求会重试，无需重启。
- `keyfile` 模式下 `keys.enc` 与 `keystore.key` 必须一起备份。

## 5. 安全边界

- router 只监听回环地址；非 `127.0.0.1` 的 `listen` 会在启动时被拒绝。
- `router.token` 为每个 CCMS_HOME 随机生成的 base64url token，权限 600；
  首次创建由 `router.token.lock` 串行化，不会读到空文件或半写入 token。
- 代理端点与 `/v1/models` 都要求 `x-api-key` 或 `Authorization: Bearer <token>`；
  不再使用固定占位 token。
- `CCMS_HOME` 权限收紧到 700。没有 `HOME` / `USERPROFILE` 且没有 `CCMS_HOME` /
  `CLAUDE_SETTINGS` 显式覆盖时直接报错，不会回退到当前目录。
- `/ccms/health` 返回 `service=ccms` 与当前 `pid`。停止前必须同时核对 health pid
  与 `router.pid`，缺失或不一致时拒绝发送信号。pidfile 只接受正数且不超过
  `i32::MAX`；Unix 发送前做范围转换，避免 PID 0 / 负数被解释为进程组。
  Windows 使用 `taskkill /PID <pid> /F`，不加 `/T`，不会连带杀进程树。
- 转发前清洗逐跳头（含 `Connection` 中列出的字段）、`cookie`、代理凭据和本地鉴权头，
  再替换为目标 provider 的鉴权。
- 非流式响应按实际读取字节限制为 16 MiB，不只信任 `Content-Length`。
- 默认不读系统 `http_proxy`；需要代理时显式设置 `CCMS_PROXY`。
  本机健康检查与 `--test router` 强制 `no_proxy`。

## 6. 可靠性与性能

- 每个 provider 一个轮询游标；第 n 次尝试使用池中下一把 key。
- Windows 启动后台 router 时临时清除父进程 stdio 的继承位，避免 `ccms --on` 的输出被管道捕获时，后台进程继续持有写端导致命令不返回；router 自身的输出仍写入 `router.log`。
- 只在响应头发给客户端之前重试。401 快速换 key，其余按 `backoff_ms × attempt`
  退避，上限 5s。全部失败返回 502；最后一次为 401 时原样透传。
- 连接超时为 `connect_timeout_ms`；等待响应头用 `response_timeout_ms` 包住
  `send()`，收到头即解除，长回答不会被截断。
- SSE 流式响应边转发边解析 usage。客户端断开时立即取消上游读取，不等待下一个 chunk；
  流一旦开始转发就不再重连，避免重复输出。
- `count_tokens` 遇 404/405 时用 `len/4` 本地估算，不记为一次请求。
- 统计写入在 `spawn_blocking` 中执行，不阻塞异步转发。`stats.json` 用临时文件 +
  rename 原子写入，按本地日期分桶，保留 180 天。文件损坏时命令报错、router 记录错误
  并保留原文件，不会用新记录覆盖。
- `router.log` 运行期按 5 MiB 轮转，保留 `router.log.1`。`--router --logs` 从文件尾
  按 8 KiB 块反向读取，最多 1 MiB 缓冲，UTF-8 解析使用 lossy。
- 健康检查只接受 `service=ccms` 且 `ok=true`；端口被占用时 router 以非零退出。

## 7. 文件与权限

配置目录为 `$CCMS_HOME`，默认 `~/.config/ccms`。

| 文件 | 权限 | 作用 |
|---|---|---|
| `routes.json` | 600 | 监听、默认档位、effort、重试、五档路由 |
| `providers.json` | 644 | 供应商端点、密钥变量、模型 |
| `keys.enc` | 600 | 加密密钥池 |
| `keystore.key` | 600 | keyfile 模式主密钥 |
| `keys.env` | 600 | 明文导入源；迁移后只剩注释 |
| `keystore.lock` | 600 | 写事务排他锁 |
| `keystore.rekey.journal` | 600 | rekey 事务状态 |
| `keys.enc.rekey.bak` | 600 | rekey 前旧密文备份 |
| `keystore.key.rekey.bak` | 600 | rekey 前旧 keyfile 备份 |
| `settings.previous.json` | 600 | `--on` 前的受管 env / 顶层 model |
| `<settings>.ccms.bak` | 600 | 每次改写前的 `settings.json` 备份 |
| `router.token` | 600 | 随机本机 API token |
| `router.token.lock` | 600 | token 创建锁 |
| `router.pid` | 644 | 当前 router 进程号 |
| `stats.json` | 644 | 按天用量统计 |
| `router.log` | 644 | 请求日志；运行期轮转 |

## 8. 兼容性与限制

- 兼容 0.1 的 `providers.json` / `routes.json` 旧字段、`keys.env` 迁移与
  `keys.enc` / `keystore.key` 格式，可直接升级；不再使用 `state.json`。
- 0.2.1 起公开命令面只保留旧式长/短 flag。0.2 的裸子命令已移除，
  传入会按未知命令失败。
- 只支持 Anthropic Messages 兼容端点，不支持 OpenAI / Gemini 原生格式。
- 不计算费用，只统计 token、请求数、重试、错误与耗时。
- 流式响应中途断线不会重试；单进程轮询游标在重启后重置。
- 平台状态：Linux / WSL 已完成构建与测试；Windows 原生已编译并运行测试；
  macOS 走 Unix 分支，尚未在真机编译。PowerShell 安装脚本只做静态契约检查。
