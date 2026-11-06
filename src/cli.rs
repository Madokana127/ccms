use clap::{Parser, Subcommand};
use std::ffi::OsString;

const AFTER_HELP: &str = "\
档位      fable / opus / sonnet / haiku / subagent（哨兵模型 ccms-<tier>）
配置      $CCMS_HOME，默认 ~/.config/ccms（三平台一致）
密钥      keys.enc + keystore.key（AES-256-GCM），密钥池变量 <PROVIDER>_API_KEYS
Claude    ~/.claude/settings.json，env 段由 ccms 独占，可用 CLAUDE_SETTINGS 覆盖
旧写法    --on/--off/--key/--stats 等仍然可用，会自动映射到子命令";

#[derive(Parser, Debug)]
#[command(
    name = "ccms",
    version,
    about = "Claude Code Model Switcher — 为每个档位自由选择供应商与模型",
    after_help = AFTER_HELP,
    arg_required_else_help = false
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 初始化配置目录（幂等；--force 重置为内置默认）
    Init {
        #[arg(long)]
        force: bool,
    },
    /// 启用路由模式：拉起 router 并写入 Claude settings.json
    On,
    /// 关闭路由模式：清理 env，回到官方 Claude
    Off,
    /// 详细状态（--json 机器可读）
    Status {
        #[arg(long)]
        json: bool,
    },
    /// 列出 provider
    List,
    /// 密钥池：<provider> <key> | list | clear | remove <n>
    Key {
        provider: String,
        action: Option<String>,
        value: Option<String>,
    },
    /// 加密密钥库：status / seal / open / rekey
    Vault {
        #[command(subcommand)]
        cmd: Option<VaultCmd>,
    },
    /// 拉取 provider 的模型列表
    Models { provider: String },
    /// 验证链路：<provider|router|all>
    Test { target: Option<String> },
    /// 路由表：list / set / unset / all
    Route {
        #[command(subcommand)]
        cmd: Option<RouteCmd>,
    },
    /// 路由器进程：start / stop / restart / status / logs / run / ensure
    Router {
        #[command(subcommand)]
        cmd: Option<RouterCmd>,
    },
    /// 用量统计（按模型 / provider / 档位）
    Stats {
        #[arg(value_name = "DAYS|reset")]
        arg: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "model|provider|tier")]
        by: Option<String>,
    },
    /// 环境自检
    Doctor,
    /// 显示版本
    Version,
}

#[derive(Subcommand, Debug, Clone, Copy)]
pub enum VaultCmd {
    /// 查看加密状态与主密钥来源
    Status,
    /// 把明文 keys.env 加密迁移
    Seal,
    /// 输出明文（谨慎）
    Open,
    /// 轮换主密钥 / 切换模式
    Rekey,
}

#[derive(Subcommand, Debug)]
pub enum RouteCmd {
    /// 查看路由表
    List,
    /// 设置某档位
    Set {
        tier: String,
        provider: String,
        model: String,
    },
    /// 删除某档位
    Unset { tier: String },
    /// 五个档位统一到一家
    All {
        provider: String,
        model: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RouterCmd {
    Start,
    Stop,
    Restart,
    Status,
    Logs {
        n: Option<usize>,
    },
    /// 前台运行（守护进程内部使用）
    Run,
    /// 未运行则静默拉起（shell 钩子用）
    Ensure,
}

pub fn print_version() {
    println!(
        "CCMS {} — Claude Code Model Switcher",
        env!("CARGO_PKG_VERSION")
    );
}

/// 0.1 版的 `--on/-a` 等写法翻译成子命令，避免老脚本失效。
pub fn translate_legacy(args: Vec<OsString>) -> Vec<OsString> {
    const TOP: &[(&str, &str)] = &[
        ("--on", "on"),
        ("-a", "on"),
        ("--off", "off"),
        ("-o", "off"),
        ("--status", "status"),
        ("--list", "list"),
        ("-l", "list"),
        ("--key", "key"),
        ("-k", "key"),
        ("--models", "models"),
        ("-m", "models"),
        ("--test", "test"),
        ("-t", "test"),
        ("--doctor", "doctor"),
        ("-d", "doctor"),
        ("--stats", "stats"),
        ("-u", "stats"),
        ("--route", "route"),
        ("-r", "route"),
        ("--router", "router"),
        ("--vault", "vault"),
        ("--help", "help"),
        ("-h", "help"),
        ("--version", "version"),
        ("-v", "version"),
    ];
    const SUB: &[&str] = &[
        "list", "set", "unset", "all", "start", "stop", "restart", "status", "logs", "seal",
        "open", "rekey", "reset", "ensure", "run",
    ];

    let mut out: Vec<OsString> = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    let Some(program) = iter.next() else {
        return out;
    };
    out.push(program);

    let mut first = true;
    for arg in iter {
        let text = arg.to_string_lossy().to_string();
        if first {
            first = false;
            if let Some((_, mapped)) = TOP.iter().find(|(flag, _)| *flag == text) {
                out.push(OsString::from(*mapped));
                continue;
            }
        }
        let trimmed = text.strip_prefix("--").unwrap_or(&text);
        if (text.starts_with("--") || text == "-l") && SUB.contains(&trimmed) {
            out.push(OsString::from(trimmed));
            continue;
        }
        out.push(arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate(input: &[&str]) -> Vec<String> {
        let args = std::iter::once("ccms")
            .chain(input.iter().copied())
            .map(OsString::from)
            .collect();
        translate_legacy(args)
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn legacy_flags_map_to_subcommands() {
        assert_eq!(translate(&["--on"]), vec!["ccms", "on"]);
        assert_eq!(
            translate(&["-k", "kimi", "sk-x"]),
            vec!["ccms", "key", "kimi", "sk-x"]
        );
        assert_eq!(
            translate(&["--route", "--list"]),
            vec!["ccms", "route", "list"]
        );
        assert_eq!(
            translate(&["--stats", "--json"]),
            vec!["ccms", "stats", "--json"]
        );
        assert_eq!(
            translate(&["--stats", "reset"]),
            vec!["ccms", "stats", "reset"]
        );
        assert_eq!(
            translate(&["--vault", "--status"]),
            vec!["ccms", "vault", "status"]
        );
    }

    #[test]
    fn modern_commands_are_untouched() {
        assert_eq!(
            translate(&["route", "set", "opus", "kimi", "kimi-k3"]),
            vec!["ccms", "route", "set", "opus", "kimi", "kimi-k3"]
        );
    }
}
