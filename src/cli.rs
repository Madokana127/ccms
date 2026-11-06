use clap::error::{ContextKind, ErrorKind};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::ffi::OsString;

use crate::paths::Paths;
use crate::term::Style;

#[derive(Parser, Debug)]
#[command(
    name = "ccms",
    disable_help_flag = true,
    disable_version_flag = true,
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
        /// 从 stdin 读取一把或多把 key（推荐，避免进入 shell history）
        #[arg(long)]
        stdin: bool,
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

/// 顶层旧 flag → 规范子命令。唯一映射表，新增命令只改这里。
const TOP_ALIASES: &[(&str, &str)] = &[
    ("--on", "on"),
    ("-a", "on"),
    ("--off", "off"),
    ("-o", "off"),
    ("--status", "status"),
    ("--list", "list"),
    ("-l", "list"),
    ("--key", "key"),
    ("-k", "key"),
    ("--vault", "vault"),
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
    ("--init", "init"),
];

/// main 的 resolve_sub：裸词、--长名、既有短名统一到同一子命令。
const ROUTE_SUBS: &[(&str, &str)] = &[
    ("list", "list"),
    ("--list", "list"),
    ("-l", "list"),
    ("set", "set"),
    ("--set", "set"),
    ("unset", "unset"),
    ("--unset", "unset"),
    ("all", "all"),
    ("--all", "all"),
];

const ROUTER_SUBS: &[(&str, &str)] = &[
    ("start", "start"),
    ("--start", "start"),
    ("stop", "stop"),
    ("--stop", "stop"),
    ("restart", "restart"),
    ("--restart", "restart"),
    ("status", "status"),
    ("--status", "status"),
    ("logs", "logs"),
    ("--logs", "logs"),
    ("run", "run"),
    ("--run", "run"),
    ("ensure", "ensure"),
    ("--ensure", "ensure"),
];

const VAULT_SUBS: &[(&str, &str)] = &[
    ("status", "status"),
    ("--status", "status"),
    ("seal", "seal"),
    ("--seal", "seal"),
    ("open", "open"),
    ("--open", "open"),
    ("rekey", "rekey"),
    ("--rekey", "rekey"),
];

pub enum Prepared {
    Help,
    Version,
    Run(Vec<OsString>),
}

fn alias(table: &[(&'static str, &'static str)], token: &str) -> Option<&'static str> {
    table
        .iter()
        .find_map(|(from, to)| (*from == token).then_some(*to))
}

/// 把 main 的旧写法规范化成 clap 子命令 token；只解析命令位，不扫描参数值。
pub fn prepare(args: Vec<OsString>) -> Result<Prepared, String> {
    let mut iter = args.into_iter();
    let program = iter.next().unwrap_or_else(|| OsString::from("ccms"));
    let rest: Vec<OsString> = iter.collect();

    let Some(first_os) = rest.first() else {
        return Ok(Prepared::Run(vec![program]));
    };
    let first = first_os.to_string_lossy();

    match first.as_ref() {
        "-h" | "--help" => return Ok(Prepared::Help),
        "-v" | "--version" => return Ok(Prepared::Version),
        _ => {}
    }

    let canonical = match alias(TOP_ALIASES, &first) {
        Some(mapped) => mapped.to_string(),
        None => {
            return Err(format!("✗ 未知命令: {first}（ccms --help/-h 看用法）"));
        }
    };

    let mut out = vec![program, OsString::from(&canonical)];
    let nested = &rest[1..];
    match canonical.as_str() {
        "route" => push_nested(
            &mut out,
            nested,
            ROUTE_SUBS,
            "list",
            "未知操作: {}（ccms --route/-r 看用法）",
        )?,
        "router" => push_nested(
            &mut out,
            nested,
            ROUTER_SUBS,
            "status",
            "未知操作: {}（ccms --router 看用法）",
        )?,
        "vault" => push_nested(
            &mut out,
            nested,
            VAULT_SUBS,
            "status",
            "未知操作: {}（ccms --help/-h 看用法）",
        )?,
        _ => out.extend(nested.iter().cloned()),
    }

    Ok(Prepared::Run(out))
}

fn push_nested(
    out: &mut Vec<OsString>,
    rest: &[OsString],
    table: &[(&'static str, &'static str)],
    default: &str,
    error_template: &str,
) -> Result<(), String> {
    let start = usize::from(rest.first().is_some_and(|token| token == "--"));
    match rest.get(start) {
        None => out.push(OsString::from(default)),
        Some(token) => {
            let text = token.to_string_lossy();
            match alias(table, &text) {
                Some(mapped) => {
                    out.push(OsString::from(mapped));
                    out.extend(rest[start + 1..].iter().cloned());
                }
                None => return Err(format!("✗ {}", error_template.replace("{}", &text))),
            }
        }
    }
    Ok(())
}

fn scrubbed_command() -> clap::Command {
    fn scrub(cmd: clap::Command) -> clap::Command {
        cmd.disable_help_flag(true)
            .disable_version_flag(true)
            .mut_subcommands(scrub)
    }
    scrub(Cli::command())
}

pub fn parse_args(argv: &[OsString]) -> Result<Cli, String> {
    let command = scrubbed_command();
    match command.try_get_matches_from(argv.iter().cloned()) {
        Ok(matches) => Cli::from_arg_matches(&matches),
        Err(err) => Err(err),
    }
    .map_err(|err| format_parse_error(&err, argv))
}

fn format_parse_error(err: &clap::Error, argv: &[OsString]) -> String {
    if matches!(
        err.kind(),
        ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand
    ) {
        let token = err
            .get(ContextKind::InvalidArg)
            .or_else(|| err.get(ContextKind::InvalidSubcommand))
            .map(|value| value.to_string());
        if let Some(token) = token {
            return format!("✗ 未知命令或参数: {token}（ccms --help/-h 看用法）");
        }
    }
    format!("✗ {}", usage_for(argv))
}

fn usage_for(argv: &[OsString]) -> &'static str {
    let command = argv.get(1).map(|value| value.to_string_lossy().to_string());
    let sub = argv.get(2).map(|value| value.to_string_lossy().to_string());
    match (command.as_deref(), sub.as_deref()) {
        (Some("key"), _) => "用法: ccms --key/-k <provider> <sk-...|--stdin|list|clear|remove <n>>",
        (Some("models"), _) => "用法: ccms --models/-m <provider>",
        (Some("test"), _) => "用法: ccms --test/-t <provider|router|all>",
        (Some("route"), Some("set")) => "用法: ccms --route/-r --set <tier> <provider> <model>",
        (Some("route"), Some("unset")) => "用法: ccms --route/-r --unset <tier>",
        (Some("route"), Some("all")) => "用法: ccms --route/-r --all <provider> [model]",
        (Some("route"), _) => "用法: ccms --route/-r <list|set|unset|all>",
        (Some("router"), Some("logs")) => "用法: ccms --router --logs [n]",
        (Some("router"), _) => "用法: ccms --router <start|stop|restart|status|logs>",
        (Some("vault"), _) => "用法: ccms --vault <--status|--seal|--open|--rekey>",
        (Some("stats"), _) => {
            "用法: ccms --stats/-u [days] [--by model|provider|tier] [--json] [reset]"
        }
        (Some("init"), _) => "用法: ccms --init [--force]",
        _ => "用法: ccms --help/-h",
    }
}

pub fn print_version() {
    println!(
        "CCMS {} — Claude Code Model Switcher",
        env!("CARGO_PKG_VERSION")
    );
}

/// main 的旧式长 flag 帮助页（不再使用 clap 的 Usage/Commands 页面）。
pub fn print_help(paths: &Paths) {
    let style = Style::detect();
    let group = |text: &str| style.cyan(text);

    println!(
        "{} — Claude Code Model Switcher  v{}",
        style.cyan("CCMS"),
        env!("CARGO_PKG_VERSION")
    );
    println!("适用于Claude Code的Provider自由切换工具");
    println!();
    println!("{}  ccms [--命令/-x] [参数]", group("用法"));
    println!("      不带参数仅显示模式与路由");
    println!();
    println!("{}", group("模式"));
    println!("  --on/-a      启用路由");
    println!("  --off/-o     关闭路由");
    println!("  --status     详细状态");
    println!();
    println!("{}", group("接入"));
    println!("  --init                     初始化配置目录（--force 重置为内置默认）");
    println!("  --key/-k");
    println!("      <provider> <sk-...>    追加到密钥池");
    println!("      <provider> --stdin     从 stdin 读取一把或多把 key（推荐）");
    println!("      <provider> list        查看密钥池");
    println!("      <provider> remove <n>  移除第 n 把");
    println!("      <provider> clear       清空密钥池");
    println!("  --models/-m");
    println!("      <provider>             列出可用模型");
    println!("  --list/-l                  列出 provider");
    println!();
    println!("{}", group("密钥库"));
    println!("  --vault");
    println!("      --status                   查看加密状态与主密钥");
    println!("      --seal                     加密现有明文密钥");
    println!("      --open                     输出明文密钥");
    println!("      --rekey                    轮换主密钥");
    println!();
    println!("{}", group("路由"));
    println!("  --route/-r");
    println!("      --list/-l                          查看路由表");
    println!("      --set <tier> <provider> <model>    修改路由");
    println!("      --unset <tier>                     删除路由");
    println!("      --all <provider> [model]           五档统一到一家");
    println!("  --router");
    println!("      --start      启动");
    println!("      --stop       停止");
    println!("      --restart    重启");
    println!("      --status     状态");
    println!("      --logs [n]   日志");
    println!();
    println!("{}", group("调试"));
    println!("  --test/-t");
    println!("      <provider|router|all>  验证链路");
    println!("  --doctor/-d                环境自检");
    println!("  --version/-v               名称与版本");
    println!("  --help/-h                  帮助");
    println!();
    println!("{}", group("统计"));
    println!("  --stats/-u [days]          用量统计（默认 7 天）");
    println!("      --by <model|provider|tier>  按维度查看");
    println!("      --json                 输出 JSON");
    println!("      reset                  清空统计");
    println!();
    println!("tier: opus / sonnet / haiku / fable / subagent");
    println!();
    println!("{}", group("文件"));
    println!("  routes.json     {}", paths.routes.display());
    println!("  providers.json  {}", paths.providers.display());
    println!("  keys.env        {}", paths.keys_env.display());
    println!("  keys.enc        {}", paths.keys_enc.display());
    println!("  keystore.key    {}", paths.keyfile.display());
    println!("  settings.json   {}", paths.settings.display());
    println!("  stats.json      {}", paths.stats.display());
    println!();
    println!("{}", group("示例"));
    println!("  ccms --init");
    println!("  printf 'sk-xxx\\n' | ccms --key kimi --stdin");
    println!("  ccms --key kimi sk-xxx");
    println!("  ccms --route --all qwen");
    println!("  ccms --stats --by model");
    println!("  ccms --on");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(input: &[&str]) -> Vec<String> {
        let args = std::iter::once("ccms")
            .chain(input.iter().copied())
            .map(OsString::from)
            .collect();
        match prepare(args).expect("prepare") {
            Prepared::Run(argv) => argv
                .into_iter()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect(),
            _ => panic!("expected run"),
        }
    }

    fn expect(input: &[&str]) -> Vec<String> {
        input.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn legacy_flags_map_to_subcommands() {
        assert_eq!(normalized(&["--on"]), expect(&["ccms", "on"]));
        assert_eq!(
            normalized(&["-k", "kimi", "sk-x"]),
            expect(&["ccms", "key", "kimi", "sk-x"])
        );
        assert_eq!(
            normalized(&["--stats", "--json"]),
            expect(&["ccms", "stats", "--json"])
        );
        assert_eq!(
            normalized(&["--stats", "reset"]),
            expect(&["ccms", "stats", "reset"])
        );
        assert_eq!(
            normalized(&["--vault", "--status"]),
            expect(&["ccms", "vault", "status"])
        );
    }

    #[test]
    fn every_legacy_top_level_form_is_known() {
        for (legacy, canonical) in [
            ("-a", "on"),
            ("--on", "on"),
            ("-o", "off"),
            ("--off", "off"),
            ("--status", "status"),
            ("-l", "list"),
            ("--list", "list"),
            ("-k", "key"),
            ("--key", "key"),
            ("-m", "models"),
            ("--models", "models"),
            ("-t", "test"),
            ("--test", "test"),
            ("-d", "doctor"),
            ("--doctor", "doctor"),
            ("-u", "stats"),
            ("--stats", "stats"),
            ("--init", "init"),
        ] {
            assert_eq!(
                normalized(&[legacy]),
                expect(&["ccms", canonical]),
                "{legacy}"
            );
        }
        for (legacy, canonical, default) in [
            ("-r", "route", "list"),
            ("--route", "route", "list"),
            ("--router", "router", "status"),
            ("--vault", "vault", "status"),
        ] {
            assert_eq!(
                normalized(&[legacy]),
                expect(&["ccms", canonical, default]),
                "{legacy}"
            );
        }
        for flag in ["-h", "--help"] {
            let args = ["ccms", flag].into_iter().map(OsString::from).collect();
            assert!(matches!(prepare(args), Ok(Prepared::Help)), "{flag}");
        }
        for flag in ["-v", "--version"] {
            let args = ["ccms", flag].into_iter().map(OsString::from).collect();
            assert!(matches!(prepare(args), Ok(Prepared::Version)), "{flag}");
        }
    }

    #[test]
    fn nested_short_and_long_flags_resolve() {
        assert_eq!(
            normalized(&["-r", "-l"]),
            expect(&["ccms", "route", "list"])
        );
        assert_eq!(
            normalized(&["-r", "--list"]),
            expect(&["ccms", "route", "list"])
        );
        assert_eq!(
            normalized(&["--route", "--set", "opus", "kimi", "m"]),
            expect(&["ccms", "route", "set", "opus", "kimi", "m"])
        );
        assert_eq!(
            normalized(&["--router", "--logs", "5"]),
            expect(&["ccms", "router", "logs", "5"])
        );
    }

    #[test]
    fn nested_values_are_not_translated_as_subcommands() {
        assert_eq!(
            normalized(&["--route", "--set", "opus", "kimi", "--list"]),
            expect(&["ccms", "route", "set", "opus", "kimi", "--list"])
        );
    }

    #[test]
    fn nested_defaults_follow_main() {
        assert_eq!(normalized(&["--route"]), expect(&["ccms", "route", "list"]));
        assert_eq!(
            normalized(&["--router"]),
            expect(&["ccms", "router", "status"])
        );
        assert_eq!(
            normalized(&["--vault"]),
            expect(&["ccms", "vault", "status"])
        );
    }

    #[test]
    fn bare_commands_are_rejected() {
        for word in MODERN_WORDS {
            let args = ["ccms", word].into_iter().map(OsString::from).collect();
            let err = prepare(args).err().expect("裸子命令必须被拒绝");
            assert!(
                err.contains(&format!("未知命令: {word}")),
                "{word} 错误不对：{err}"
            );
        }
    }

    #[test]
    fn internal_router_entry_is_reachable_only_via_legacy_flag() {
        assert_eq!(
            normalized(&["--router", "--run"]),
            expect(&["ccms", "router", "run"])
        );
        assert_eq!(
            normalized(&["--router", "--ensure"]),
            expect(&["ccms", "router", "ensure"])
        );
        assert_eq!(
            normalized(&["--router", "run"]),
            expect(&["ccms", "router", "run"])
        );
        let args = ["ccms", "router", "run"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(prepare(args).is_err(), "裸 router run 不得公开");
    }

    #[test]
    fn unknown_nested_is_rejected() {
        let args = ["ccms", "--route", "frob"]
            .into_iter()
            .map(OsString::from)
            .collect();
        let err = match prepare(args) {
            Err(err) => err,
            Ok(_) => panic!("未知嵌套操作必须被拒绝"),
        };
        assert!(err.contains("未知操作: frob"), "{err}");
    }

    const MODERN_WORDS: &[&str] = &[
        "init", "on", "off", "status", "list", "key", "vault", "models", "test", "route", "router",
        "stats", "doctor", "version", "help",
    ];
}
