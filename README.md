# CCMS 0.1.0 — Claude Code Model Switcher

Claude Code 的本地模型路由与切换工具。Claude Code 只连接 `127.0.0.1:8788`，
CCMS 按请求档位把请求转发到配置好的 Anthropic 兼容端点，并处理密钥池、重试和用量统计。

## 工作方式

共五个逻辑档位：`fable`、`opus`、`sonnet`、`haiku`、`subagent`。
Claude Code 使用哨兵模型名 `ccms-<tier>`，路由器将其替换为 `routes.json`
中对应档位的真实 `<provider>/<model>`。

- `Mode`：`Claude` 为官方直连，`Router` 为本地路由。
- `Default`：默认档位与 effort。
- 路由表与 provider 均可配置，不绑定特定供应商或模型。

## 快速开始

```bash
ccms --key <provider> <api-key>
ccms --test <provider>
ccms --on
ccms --test router
claude
```

`--key` 的参数名以 `providers.json` 中已配置的 provider 为准。
修改路由或 provider 配置后，需要重启路由器并新开一个 `claude` 进程。

## 常用命令

| 目的 | 命令 |
|---|---|
| 查看模式和路由 | `ccms`、`ccms --status` |
| 启用 / 关闭路由 | `ccms --on/-a`、`ccms --off/-o` |
| 添加 / 查看 / 清空密钥 | `ccms --key/-k <provider> <api-key>`、`... list`、`... clear` |
| 查看 provider | `ccms --list/-l` |
| 拉取端点模型列表 | `ccms --models/-m <provider>` |
| 查看路由表 | `ccms --route/-r --list/-l` |
| 修改 / 删除路由 | `ccms --route/-r --set <tier> <provider> <model>`、`--unset <tier>` |
| 五档统一到一家 | `ccms --route/-r --all <provider> [model]` |
| 管理路由器 | `ccms --router`（start / stop / restart / status / logs [n]） |
| 验证链路 | `ccms --test/-t`（provider 或 router） |
| 环境自检 | `ccms --doctor/-d` |
| 用量统计 | `ccms --stats/-u [days]`、`--json`、`reset` |

完整参数见 `ccms --help`。

## 配置与密钥

`routes.json` 管理监听地址、默认档位、effort、重试参数和五档路由。
路由器只在启动时读取该文件，修改后执行：

```bash
ccms --router --restart
```

密钥池支持为每个 provider 配置多把密钥，以英文逗号分隔；请求失败时会轮换。
真实密钥只保存在 `~/.config/ccms/keys.env`（权限 600）。`ccms --key` 会立即重新加载，
旧式单密钥写法仍然兼容。

重试行为由 `routes.json` 的 `retry` 段控制：

```json
"retry": {
  "attempts": 3,
  "backoff_ms": 400,
  "timeout_ms": 120000,
  "connect_timeout_ms": 15000,
  "retry_on": [401, 403, 408, 429, 500, 502, 503, 504]
}
```

重试只发生在响应头发送给客户端之前；流式输出开始后不再重连，避免重复内容。

## 用量统计

记录请求数、输入/输出 token、缓存 token 和重试次数，按本地日期保留 180 天：

```bash
ccms --stats           # 今日 + 最近 7 天
ccms --stats 30        # 最近 30 天
ccms --stats --json    # 机器可读
ccms --stats reset     # 清空
```

统计只包含 token 与请求数，不计算费用。

## 安装

依赖：`bash`、`node`、`jq`、`curl`，以及写入 `/usr/local` 的权限。
适用于 Linux 和 WSL。

```bash
git clone https://github.com/Madokana127/ccms.git
cd ccms
sudo ./install.sh
./scripts/validate.sh
```

安装脚本会部署 CLI、路由器和默认配置；不会覆盖已有的
`routes.json`、`providers.json` 或 `keys.env`。真实密钥只保存在
`~/.config/ccms/keys.env`，不进入 Git。

源码结构：

```text
bin/ccms                 CLI
lib/ccms/router.js       路由器
lib/ccms/stats.js        用量统计
config/                 默认配置与密钥模板
install.sh              安装
scripts/validate.sh     隔离验证
```

## 注意

- 路由器只监听 `127.0.0.1`，不要改为对外地址。
- 路由模式下 Claude Code 侧使用占位 token，真实密钥只在 `keys.env`。
- 上游必须兼容 Anthropic Messages 协议。
- 切换后必须新开 Claude Code 进程。
- 第三方密钥的计费与额度由对应服务决定。
- 非官方用法下，工具调用、缓存和会话压缩的表现可能与原生 Claude 不同。
