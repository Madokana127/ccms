# CCMS 命令参考

## 约定

- 裸 `ccms` 显示首页：模式、默认档位、五档路由与密钥概览。
- 所有公开操作均使用长/短 flag；**不提供裸子命令**。
- 文本中的 `--route/-r` 表示二选一；可执行示例一律写实际形式。
- 嵌套操作接受裸词、`--长名` 和既有短名，但必须跟在所属顶层 flag 之后。
- 嵌套缺省值：`--route` = `list`，`--router` = `status`，`--vault` = `status`。
- 短名：`-a/--on`、`-o/--off`、`-l/--list`、`-k/--key`、`-m/--models`、`-t/--test`、
  `-d/--doctor`、`-u/--stats`、`-r/--route`、`-v/--version`、`-h/--help`。
  `--status`、`--vault`、`--router`、`--init` 没有短名。

## 模式

| 命令 | 说明 |
|---|---|
| `ccms` | 首页；不启动进程、不修改配置 |
| `ccms --on` / `ccms -a` | 启动 router，写入 Claude `settings.json`，进入路由模式 |
| `ccms --off` / `ccms -o` | 移除 CCMS 管理的 env，恢复原设置，回到官方端点 |
| `ccms --status [--json]` | 详细状态；`--json` 输出机器可读 JSON |

```bash
ccms --on
ccms --status
ccms --status --json
ccms --off
```

`--on` 要求默认档位已经配置路由，否则会提示先用 `--route --set` 设置。

## 接入

| 命令 | 说明 |
|---|---|
| `ccms --init [--force]` | 初始化配置目录；`--force` 重置为内置默认 |
| `ccms --list` / `ccms -l` | 列出 provider、端点、密钥数量与默认模型 |
| `ccms --models <provider>` / `ccms -m <provider>` | 请求该端点的 `/v1/models` 并列出模型 |
| `ccms --test <provider\|router\|all>` / `ccms -t ...` | 验证单家 provider、本机 router 或全部 provider |

```bash
ccms --init
ccms --list
ccms --models kimi
ccms --test deepseek
ccms --test router
ccms --test all
```

`--test <provider>` 直接请求供应商端点；`--test router` 通过本机 router 走完整链路，
需要 router 正在运行；`--test all` 逐个测试已配置的 provider。

## 密钥库

| 命令 | 说明 |
|---|---|
| `ccms --key <provider> <key>` | 追加一把 key |
| `ccms --key <provider> --stdin` | 从 stdin 读取一把或多把 key（推荐，避免进入 shell history） |
| `ccms --key <provider> list` | 查看密钥池（掩码显示）；省略动作时默认 `list` |
| `ccms --key <provider> remove <n>` | 移除第 n 把（从 1 开始） |
| `ccms --key <provider> clear` | 清空该 provider 的密钥池 |

```bash
printf 'sk-aaaa\nsk-bbbb\n' | ccms --key kimi --stdin
ccms --key kimi sk-cccc
ccms --key kimi list
ccms --key kimi remove 2
ccms --key kimi clear
```

| 命令 | 说明 |
|---|---|
| `ccms --vault --status` | 查看加密状态、主密钥来源与条目数（缺省动作） |
| `ccms --vault --seal` | 把明文 `keys.env` 迁移进加密库 |
| `ccms --vault --open` | 输出明文密钥，谨慎使用 |
| `ccms --vault --rekey` | 轮换主密钥或切换模式 |

```bash
ccms --vault --status
ccms --vault --seal
ccms --vault --rekey
```

`--key` 的写入是幂等的：已存在的 key 不会重复加入。`--stdin` 只用于新增，
不能与 `list`、`remove`、`clear` 组合。空 stdin 会以非零退出。

## 路由

| 命令 | 说明 |
|---|---|
| `ccms --route --list` / `ccms -r -l` | 查看五档路由（缺省动作） |
| `ccms --route --set <tier> <provider> <model>` | 设置某档位 |
| `ccms --route --unset <tier>` | 删除某档位 |
| `ccms --route --all <provider> [model]` | 五档统一到一家；省略 model 时用该家默认模型 |

```bash
ccms --route --list
ccms --route --set opus kimi kimi-k3.1-pro
ccms --route --unset haiku
ccms --route --all deepseek
ccms --route --all qwen qwen3-coder-plus
```

档位固定为 `fable / opus / sonnet / haiku / subagent`。修改后 router 热加载，
无需重启。未知档位或未知 provider 会以非零退出。

## 进程

| 命令 | 说明 |
|---|---|
| `ccms --router --start` | 启动 router（缺省动作是 `status`） |
| `ccms --router --stop` | 停止 router |
| `ccms --router --restart` | 重启 router |
| `ccms --router --status` | 查看运行状态、版本、默认档位与路由 |
| `ccms --router --logs [n]` | 查看日志末尾 n 行，默认 40 |

```bash
ccms --router --start
ccms --router --status
ccms --router --logs 100
ccms --router --restart
ccms --router --stop
```

router 只监听回环地址。`--on` 会在需要时自动启动 router，通常不必手动执行
`--router --start`。

## 诊断

| 命令 | 说明 |
|---|---|
| `ccms --doctor` / `ccms -d` | 检查配置、密钥库、Claude 路径与 router 状态 |
| `ccms --version` / `ccms -v` | 输出 `CCMS 0.2.1 — Claude Code Model Switcher` |
| `ccms --help` / `ccms -h` | 显示长 flag 帮助页 |

```bash
ccms --doctor
ccms --version
ccms --help
```

## 统计

| 命令 | 说明 |
|---|---|
| `ccms --stats [days]` / `ccms -u [days]` | 用量统计，默认最近 7 天 |
| `ccms --stats --by <model\|provider\|tier>` | 只看某个维度（默认三个维度都显示） |
| `ccms --stats --json` | 输出 JSON |
| `ccms --stats reset` | 清空统计（TTY 下需要确认） |

```bash
ccms --stats
ccms --stats 30 --by model
ccms --stats --json
ccms --stats reset
```

统计维度为 `total`、`providers`、`tiers`、`models`，包含请求数、token、缓存 token、
重试、错误与耗时。

## 错误与退出码

- 成功为 `0`；未知命令、未知操作、未知参数、缺少参数或运行时错误均为非零。
- 错误写到 stderr，通常只有一行中文提示，并提示 `ccms --help/-h 看用法`。
- 公共帮助与错误提示均为中文，不出现英文 `Usage:` / `Commands:` 页面。
- 嵌套操作写错时提示 `未知操作: <token>`，并指出对应的顶层 flag。

```text
✗ 未知命令: frob（ccms --help/-h 看用法）
✗ 未知操作: frob（ccms --route/-r 看用法）
```
