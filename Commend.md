# Alkanet 命令参考 (`aknt`)

## 约定

- 无参数运行 `aknt` 显示看板：模式、默认档位、五档路由与密钥概览。
- 所有操作均为标准子命令，可用 `aknt <command> --help` 查看单条用法。

## 模式与状态

| 命令 | 说明 |
|---|---|
| `aknt` | 看板；不启动进程、不修改路由 |
| `aknt on` | 启动 `alkanetd`，写入 Claude `settings.json`，进入路由模式 |
| `aknt off` | 移除 Alkanet 管理的 env，恢复原设置，回到官方端点 |
| `aknt status [--check] [--json]` | 查看运行状态与密钥库；`--check` 附带环境/权限自检，`--json` 输出 JSON |

## 配置与路由

| 命令 | 说明 |
|---|---|
| `aknt use <provider> [model]` | 将 `fable/opus/sonnet/haiku/subagent` 五档统一切换到同一 provider |
| `aknt config` | 查看当前生效配置、路由与 provider 列表 |
| `aknt config path` | 输出 `config.toml` 路径（缺失时自动初始化） |
| `aknt config edit` | 用 `$EDITOR` 打开 `config.toml`，保存后校验 TOML 语法 |
| `aknt config reset --force` | 重置 `config.toml` 为内置默认 |

## 密钥池与加密库

底层加密与跨进程文件锁由 `alkanetd vault` 执行：

| 命令 | 说明 |
|---|---|
| `aknt key add <provider> [sk-...] [--stdin]` | 添加一把或多把 key（推荐 `--stdin`） |
| `aknt key ls <provider>` | 列出脱敏密钥池 |
| `aknt key rm <provider> <n>` | 移除第 n 把 key |
| `aknt key clear <provider>` | 清空该 provider 的密钥池 |
| `aknt key seal` | 将明文 `keys.env` 迁移加密为 `keys.enc` |
| `aknt key rekey` | 轮换主密钥或切换密钥库模式 |

## 守护进程与诊断

| 命令 | 说明 |
|---|---|
| `aknt router start` | 启动 `alkanetd` |
| `aknt router stop` | 停止 `alkanetd` |
| `aknt router restart` | 重启 `alkanetd` |
| `aknt router logs [n]` | 查看最近 n 行日志（默认 40） |
| `aknt models <provider>` | 拉取该供应商 `/v1/models` 列表 |
| `aknt test [router\|all\|<provider>]` | 发送最小请求验证链路（默认 `router`） |

## 统计、补全与版本

| 命令 | 说明 |
|---|---|
| `aknt stats [days] [--by model\|provider\|tier] [--json]` | 查看用量统计（默认 7 天） |
| `aknt stats reset` | 清空统计数据（需交互确认） |
| `aknt completion <bash\|zsh\|fish\|powershell>` | 生成 shell 自动补全脚本 |
| `aknt version` | 输出版本信息 |
