# CCMS 0.2.1 — Claude Code Model Switcher

CCMS 是 Claude Code 的本地前置路由器：Claude Code 只连接 `127.0.0.1:8788`，
CCMS 按档位把请求转发到不同供应商的 Anthropic 兼容端点，并负责密钥池、重试与用量统计。
单个二进制，Windows / macOS / Linux 通用。

## 安装

需要 Rust 1.82+（<https://rustup.rs>）和平台常规 C 链接器。

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

安装脚本会构建 release、安装二进制、配置 PATH，并执行 `ccms --init`。
已有配置不会被覆盖。卸载用安装脚本的 `--uninstall`（配置保留）。

## 快速开始

```bash
ccms --init
printf 'sk-xxxx\n' | ccms --key deepseek --stdin
ccms --route --all deepseek deepseek-v4-pro
ccms --on
ccms --test router
claude
```

`--on` 会启动 router 并改写 Claude Code 的 `settings.json`；新开一个 `claude`
会话生效。关闭路由用 `ccms --off`。

## 基本概念

- **档位**：`fable / opus / sonnet / haiku / subagent` 五档独立路由。
- **provider / model**：每档绑定一个供应商与模型，例如 `qwen/qwen3-coder-plus`。
- **路由模式**：`--on` 使用 CCMS 的本地 router；`--off` 回到 Claude 官方端点。
- **密钥池**：同一 provider 可放多把 key，失败时自动换 key 重试。

## 常用命令

| 目的 | 命令 |
|---|---|
| 首页（模式 / 档位 / 路由 / 密钥） | `ccms` |
| 初始化配置 | `ccms --init [--force]` |
| 添加密钥（推荐 stdin） | `printf 'sk-xxxx\n' \| ccms --key <provider> --stdin` |
| 查看密钥池 | `ccms --key <provider> list` |
| 修改路由 | `ccms --route --set <tier> <provider> <model>` |
| 查看路由 | `ccms --route --list` |
| 启用 / 关闭路由 | `ccms --on` / `ccms --off` |
| 验证整条链路 | `ccms --test router` |
| router 状态 / 日志 | `ccms --router --status` / `ccms --router --logs [n]` |
| 用量统计 | `ccms --stats [days] [--by model\|provider\|tier]` |
| 环境自检 | `ccms --doctor` |
| 帮助 / 版本 | `ccms --help` / `ccms --version` |

所有公开操作均为长/短 flag，不提供裸子命令。短名与长名等价，例如
`ccms -r -l` 等同 `ccms --route --list`。

## 文档

- [Commend.md](Commend.md)：完整命令参考（全部 flag、参数、默认值、示例）。
- [Detail.md](Detail.md)：架构、配置、密钥库、安全边界与可靠性说明。
