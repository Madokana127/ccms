# Alkanet 1.0.0 (`aknt`)

Alkanet 是面向 Claude Code 的本地多供应商前置路由器：Claude Code 只连接本地 `127.0.0.1:8788`，守护进程 `alkanetd` 按档位将请求转发到不同供应商的 Anthropic 兼容端点，并负责加密密钥池、轮询重试与用量统计。

- **CLI (`aknt`)**：Go + Cobra 实现的标准子命令界面。
- **守护进程 (`alkanetd`)**：Rust 实现的异步 HTTP 代理与 AES-256-GCM 加密密钥库。
- **配置 (`config.toml`)**：单文件 TOML 配置，支持热加载。

## 安装

需要 Rust 1.82+（<https://rustup.rs>）、Go 1.22+（<https://go.dev/dl/>）和平台常规 C 链接器。

**Linux / macOS / WSL**

```bash
git clone https://github.com/Madokana127/ccms.git
cd ccms
./install.sh
```

**Windows（PowerShell）**

```powershell
git clone https://github.com/Madokana127/ccms.git
cd ccms
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

安装脚本会构建 `alkanetd`（Rust）与 `aknt`（Go）、安装二进制、配置 PATH，并初始化 `~/.config/alkanet/config.toml`（已有配置不会覆盖）。卸载使用 `--uninstall`（或 `-Uninstall`），配置目录保留。

## 快速开始

```bash
printf 'sk-xxxx\n' | aknt key add deepseek --stdin
aknt use deepseek deepseek-v4-pro
aknt on
aknt test router
claude
```

`aknt on` 会启动 `alkanetd` 并写入 Claude Code 的 `settings.json`；新开一个 `claude` 会话生效。关闭路由执行 `aknt off`。

## 基本概念

- **档位**：`fable / opus / sonnet / haiku / subagent` 五档独立路由。
- **config.toml**：集中管理监听地址、默认档位、重试策略、五档路由与供应商列表，修改保存后自动热加载。
- **密钥池**：同一 provider 可放多把 key（AES-256-GCM 加密存储），失败时自动轮换重试。

## 常用命令

| 目的 | 命令 |
|---|---|
| 看板（模式 / 档位 / 路由 / 密钥） | `aknt` |
| 添加密钥（推荐 stdin） | `printf 'sk-xxxx\n' \| aknt key add <provider> --stdin` |
| 查看密钥池 | `aknt key ls <provider>` |
| 五档统一切换 | `aknt use <provider> [model]` |
| 查看 / 编辑配置 | `aknt config` / `aknt config edit` |
| 启用 / 关闭路由 | `aknt on` / `aknt off` |
| 状态与自检 | `aknt status [--check] [--json]` |
| 验证链路 | `aknt test [router\|all\|<provider>]` |
| 守护进程控制 / 日志 | `aknt router <start\|stop\|restart\|logs [n]>` |
| 用量统计 | `aknt stats [days] [--by model\|provider\|tier]` |
| 补全 / 版本 | `aknt completion <shell>` / `aknt version` |

## 文档

- [Commend.md](Commend.md)：完整命令参考。
- [Detail.md](Detail.md)：架构、配置、密钥库、安全边界与可靠性说明。
