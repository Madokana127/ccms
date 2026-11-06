//! CLI 契约测试：直接跑真实二进制，只用临时 CCMS_HOME / CLAUDE_SETTINGS。
//! 不启动 router、不访问任何外部网络。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ccms");
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Ctx {
    dir: PathBuf,
    home: PathBuf,
}

impl Ctx {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ccms-cli-{}-{unique}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        Self {
            home: dir.join("ccms"),
            dir,
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env("CCMS_HOME", &self.home)
            .env("CLAUDE_SETTINGS", self.dir.join(".claude/settings.json"))
            .env("NO_COLOR", "1")
            .env_remove("CCMS_KEYSTORE_PASSPHRASE");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("spawn ccms")
    }

    fn run_with_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .cmd(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn ccms");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait ccms")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "ccms {args:?} 应成功，实际 {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn fail(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            !output.status.success(),
            "ccms {args:?} 应以非零退出\nstdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            !stderr.contains("Usage:"),
            "泄漏了 clap 英文 Usage：\n{stderr}"
        );
        assert!(
            !stderr.contains("Commands:"),
            "泄漏了 clap 英文 Commands：\n{stderr}"
        );
        stderr
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn has_ansi(text: &str) -> bool {
    text.contains('\u{1b}')
}

fn lines(output: &str) -> Vec<&str> {
    output.lines().collect()
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

struct RouterGuard<'a>(&'a Ctx);

impl Drop for RouterGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.run(&["--router", "--stop"]);
    }
}

#[test]
fn bare_ccms_is_dashboard_with_main_layout() {
    let ctx = Ctx::new("dashboard");
    let output = ctx.ok(&[]);
    assert!(!has_ansi(&output), "非 TTY 不应有 ANSI：{output:?}");

    let lines = lines(&output);
    assert!(lines.len() >= 10, "dashboard 输出过短：\n{output}");
    assert!(
        lines[0].starts_with("Mode "),
        "第一行应为 Mode：{:?}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("Default "),
        "第二行应为 Default：{:?}",
        lines[1]
    );
    assert_eq!(lines[2], "Route List");

    let tiers: Vec<&str> = lines[3..8]
        .iter()
        .map(|line| line.trim_start().split("->").next().unwrap_or("").trim())
        .collect();
    assert_eq!(tiers, ["Fable", "Opus", "Sonnet", "Haiku", "subagent"]);
    for line in &lines[3..8] {
        assert!(line.contains("->"), "dashboard 箭头必须是 ->：{line:?}");
        assert!(!line.contains('→'), "dashboard 不应使用 →：{line:?}");
    }

    assert!(lines.contains(&"API Key"), "缺少 API Key 段：\n{output}");
    assert_eq!(
        lines.last().copied(),
        Some("提示  ccms --help/-h"),
        "dashboard 最后一行必须是帮助提示：\n{output}"
    );
}

#[test]
fn help_restores_legacy_long_flag_page() {
    let ctx = Ctx::new("help");
    for flag in ["-h", "--help"] {
        let output = ctx.ok(&[flag]);
        assert!(!has_ansi(&output), "非 TTY 不应有 ANSI");
        assert!(
            output.starts_with("CCMS — Claude Code Model Switcher  v0.2.1\n"),
            "帮助头部不对：\n{output}"
        );
        assert!(
            output.contains("适用于Claude Code的Provider自由切换工具"),
            "缺少旧 tagline"
        );
        assert!(output.contains("ccms [--命令/-x] [参数]"), "缺少旧式用法行");
        assert!(output.contains("不带参数仅显示模式与路由"));
        assert!(
            !output.contains("Commands:"),
            "不得出现 clap Commands: 页面"
        );
        assert!(!output.contains("Usage:"), "不得出现 clap Usage: 页面");
        assert!(!output.contains("-V"), "不得出现大写短 flag -V");

        for expected in [
            "--on/-a",
            "--off/-o",
            "--status",
            "--list/-l",
            "--key/-k",
            "--stdin",
            "--vault",
            "--models/-m",
            "--test/-t",
            "--doctor/-d",
            "--stats/-u",
            "--route/-r",
            "--router",
            "--init",
            "--help/-h",
            "--version/-v",
            "remove <n>",
            "--by",
            "all",
        ] {
            assert!(output.contains(expected), "帮助缺少 {expected}：\n{output}");
        }
    }
}

#[test]
fn version_is_single_legacy_line() {
    let ctx = Ctx::new("version");
    for flag in ["-v", "--version"] {
        let output = ctx.ok(&[flag]);
        assert_eq!(
            output, "CCMS 0.2.1 — Claude Code Model Switcher\n",
            "{flag} 输出不对"
        );
    }
}

#[test]
fn nested_short_and_long_flags_resolve_like_main() {
    let ctx = Ctx::new("nested");

    // 同一个子命令的裸词 / --长名 / 短名都应解析到 route list。
    for args in [
        vec!["-r", "-l"],
        vec!["-r", "--list"],
        vec!["--route", "list"],
        vec!["--route", "--list"],
        vec!["-r", "list"],
    ] {
        let output = ctx.ok(&args);
        assert!(
            output.contains("路由表"),
            "ccms {args:?} 应显示路由表：\n{output}"
        );
    }

    for args in [vec!["-r"], vec!["--route"]] {
        let output = ctx.ok(&args);
        assert!(
            output.contains("路由表"),
            "缺省 route=list 失败：\n{output}"
        );
    }

    let output = ctx.ok(&["--route", "--set", "opus", "kimi", "cli-test-model"]);
    assert!(output.contains("opus 已更新"), "旧式 set 失败：\n{output}");
    assert!(output.contains("kimi/cli-test-model"));

    let output = ctx.ok(&["-r", "--unset", "opus"]);
    assert!(
        output.contains("已删除路由 opus"),
        "旧式 unset 失败：\n{output}"
    );

    let output = ctx.ok(&["--route", "--all", "kimi", "kimi-k3"]);
    assert!(
        output.contains("五个档位已统一到 kimi/kimi-k3"),
        "旧式 all 失败：\n{output}"
    );

    let output = ctx.ok(&["--router", "--logs", "5"]);
    assert!(
        output.contains("暂无日志"),
        "旧式 router logs 失败：\n{output}"
    );

    let output = ctx.ok(&["--vault", "--status"]);
    assert!(
        output.contains("密钥库"),
        "旧式 vault status 失败：\n{output}"
    );

    // vault 缺省 = status
    let output = ctx.ok(&["--vault"]);
    assert!(
        output.contains("密钥库"),
        "缺省 vault=status 失败：\n{output}"
    );
}

#[test]
fn top_level_legacy_forms_stay_usable() {
    let ctx = Ctx::new("top");

    assert!(ctx.ok(&["--init"]).contains("配置目录"));
    assert!(ctx.ok(&["--list"]).contains("PROVIDER"));

    let output = ctx.ok(&["--stats", "--json"]);
    let _: serde_json::Value = serde_json::from_str(&output).expect("stats --json 应是 JSON");

    assert!(ctx.ok(&["--route", "--list"]).contains("路由表"));
    assert!(ctx.ok(&["--router", "--logs", "3"]).contains("暂无日志"));
    assert!(ctx.ok(&["--off"]).contains("已关闭路由"));

    let output = ctx.ok(&["--key", "kimi", "sk-cli-dummy"]);
    assert!(output.contains("密钥池"));
    let listed = ctx.ok(&["--key", "kimi", "list"]);
    assert!(
        listed.contains("sk-cli"),
        "旧式 key list 应显示掩码：\n{listed}"
    );
}

#[test]
fn every_legacy_top_level_flag_reaches_its_command() {
    let ctx = Ctx::new("legacy-all");
    ctx.ok(&["--init"]);
    ctx.ok(&["--init", "--force"]);

    let port = free_port();
    let routes_path = ctx.home.join("routes.json");
    let mut routes: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&routes_path).unwrap()).unwrap();
    routes["listen"] = serde_json::json!(format!("127.0.0.1:{port}"));
    std::fs::write(&routes_path, serde_json::to_vec(&routes).unwrap()).unwrap();
    let _guard = RouterGuard(&ctx);

    assert!(ctx.ok(&["-l"]).contains("PROVIDER"));
    assert!(ctx.ok(&["--list"]).contains("PROVIDER"));
    assert!(ctx.ok(&["-k", "kimi", "sk-all-123456"]).contains("密钥池"));
    assert!(ctx.ok(&["--key", "kimi", "list"]).contains("sk-all"));
    assert!(ctx.ok(&["-r"]).contains("路由表"));
    assert!(ctx.ok(&["--route", "--list"]).contains("路由表"));
    assert!(ctx.ok(&["--vault"]).contains("密钥库"));
    assert!(ctx
        .ok(&["-u", "--json"])
        .parse::<serde_json::Value>()
        .is_ok());
    assert!(ctx
        .ok(&["--stats", "--json"])
        .parse::<serde_json::Value>()
        .is_ok());
    assert!(ctx.ok(&["-d"]).contains("环境"));
    assert!(ctx.ok(&["--doctor"]).contains("环境"));
    assert!(ctx.ok(&["-v"]).contains("0.2.1"));
    assert!(ctx.ok(&["--version"]).contains("0.2.1"));
    assert!(ctx.ok(&["-h"]).contains("v0.2.1"));
    assert!(ctx.ok(&["--help"]).contains("v0.2.1"));

    // models / test 先经过参数解析；未知 provider 证明已进入对应命令。
    for args in [
        vec!["-m", "ghost"],
        vec!["--models", "ghost"],
        vec!["-t", "ghost"],
        vec!["--test", "ghost"],
    ] {
        let stderr = ctx.fail(&args);
        assert!(
            stderr.contains("未知 provider"),
            "ccms {args:?} 应进入 provider 校验：{stderr:?}"
        );
    }

    // --on/-a 与 --off/-o 走同一条模式切换路径。
    assert!(ctx.ok(&["--on"]).contains("路由模式已启用"));
    assert!(ctx.ok(&["--off"]).contains("已关闭路由"));
    assert!(ctx.ok(&["-a"]).contains("路由模式已启用"));
    assert!(ctx.ok(&["-o"]).contains("已关闭路由"));

    // 嵌套进程操作：start / restart / status / logs / stop。
    assert!(ctx.ok(&["--router", "--start"]).contains("已在运行"));
    assert!(ctx.ok(&["--router", "--restart"]).contains("已启动"));
    assert!(ctx.ok(&["--router", "--status"]).contains("运行中"));
    assert!(ctx.ok(&["--router", "--logs", "3"]).contains("listening"));
    assert!(ctx.ok(&["--router", "--stop"]).contains("已停止"));
}

#[test]
fn stats_json_and_legacy_flags_cover_same_features() {
    let ctx = Ctx::new("legacy-coverage");

    let output = ctx.ok(&["--stats", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&output).expect("合法 JSON");
    assert!(value.get("days").is_some());

    assert!(ctx.ok(&["-u", "--by", "model"]).contains("用量"));

    ctx.ok(&["--key", "kimi", "sk-legacy-123456"]);
    assert!(ctx.ok(&["-k", "kimi", "list"]).contains("sk-leg"));

    assert!(ctx
        .ok(&["--route", "--set", "haiku", "deepseek", "deepseek-flash"])
        .contains("haiku 已更新"));
    assert!(ctx
        .ok(&["-r", "--list"])
        .contains("deepseek/deepseek-flash"));
    assert!(ctx
        .ok(&["--route", "--unset", "haiku"])
        .contains("已删除路由 haiku"));

    assert!(ctx.ok(&["--vault", "--status"]).contains("密钥库"));
    assert!(ctx.ok(&["--router", "--logs", "2"]).contains("暂无日志"));
    assert!(ctx.ok(&["--init"]).contains("配置目录"));
}

#[test]
fn concurrent_key_adds_keep_every_dummy_key() {
    let ctx = Ctx::new("concurrent-add");
    ctx.ok(&["--init"]);

    const ADDERS: usize = 8;
    std::thread::scope(|scope| {
        for index in 0..ADDERS {
            let ctx = &ctx;
            scope.spawn(move || {
                let key = format!("sk-concurrent-{index:02}-tail{index:04}");
                let output = ctx.run(&["--key", "kimi", &key]);
                assert!(
                    output.status.success(),
                    "并发新增 {key} 失败\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            });
        }
    });

    let listed = ctx.ok(&["-k", "kimi", "list"]);
    assert!(
        listed.contains(&format!("共 {ADDERS} 把")),
        "并发新增后不得丢 key：\n{listed}"
    );
    for index in 0..ADDERS {
        assert!(
            listed.contains(&format!("{index:04}")),
            "缺少第 {index} 把 dummy key：\n{listed}"
        );
    }
}

#[test]
fn rekey_and_key_adds_serialize_on_transaction_lock() {
    let ctx = Ctx::new("concurrent-rekey");
    ctx.ok(&["--init"]);
    ctx.ok(&["--key", "kimi", "sk-base-0000"]);

    const ADDERS: usize = 6;
    std::thread::scope(|scope| {
        for index in 0..ADDERS {
            let ctx = &ctx;
            scope.spawn(move || {
                let key = format!("sk-rekey-{index:02}-tail{index:04}");
                let output = ctx.run(&["--key", "kimi", &key]);
                assert!(
                    output.status.success(),
                    "rekey 并发新增 {key} 失败\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            });
        }
        for _ in 0..3 {
            let ctx = &ctx;
            scope.spawn(move || {
                let output = ctx.run(&["--vault", "--rekey"]);
                assert!(
                    output.status.success(),
                    "并发 rekey 失败\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            });
        }
    });

    let listed = ctx.ok(&["-k", "kimi", "list"]);
    assert!(
        listed.contains(&format!("共 {} 把", ADDERS + 1)),
        "rekey 与更新并发后不得丢 key：\n{listed}"
    );
}

#[test]
fn key_stdin_reads_multiple_keys_without_echoing_plaintext() {
    let ctx = Ctx::new("stdin");
    let secrets = ["sk-stdin-alpha-1234567890", "sk-stdin-beta-0987654321"];

    let output = ctx.run_with_stdin(
        &["--key", "kimi", "--stdin"],
        &format!("{}\n{}\n", secrets[0], secrets[1]),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "key --stdin 应成功\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    for secret in secrets {
        assert!(!stdout.contains(secret), "stdout 不得回显明文 key");
        assert!(!stderr.contains(secret), "stderr 不得回显明文 key");
    }

    let ciphertext = std::fs::read_to_string(ctx.home.join("keys.enc")).expect("keys.enc");
    for secret in secrets {
        assert!(!ciphertext.contains(secret), "密文文件不得出现明文 key");
    }

    let listed = ctx.ok(&["--key", "kimi", "list"]);
    assert!(listed.contains("sk-std"), "列表应显示掩码：\n{listed}");
    for secret in secrets {
        assert!(!listed.contains(secret), "掩码不得泄露完整 key：\n{listed}");
    }

    // 旧式 --key/-k 走同一条 stdin 路径。
    let legacy_secret = "sk-legacy-gamma-1122334455";
    let output = ctx.run_with_stdin(&["-k", "kimi", "--stdin"], legacy_secret);
    assert!(
        output.status.success(),
        "旧式 --key --stdin 应成功：\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(legacy_secret),
        "旧式 stdin 也不得回显明文"
    );
    let listed = ctx.ok(&["--key", "kimi", "list"]);
    assert!(!listed.contains(legacy_secret));

    // 空 stdin 必须失败，而不是写入空 key。
    let output = ctx.run_with_stdin(&["-k", "kimi", "--stdin"], "  \n");
    assert!(!output.status.success(), "空 stdin 应非零退出");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("stdin 为空"),
        "空 stdin 应给出明确错误：\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bare_commands_are_all_rejected() {
    let ctx = Ctx::new("bare-commands");

    for word in [
        "init", "on", "off", "status", "list", "key", "vault", "models", "test", "route", "router",
        "stats", "doctor", "version", "help",
    ] {
        let stderr = ctx.fail(&[word]);
        assert_eq!(
            stderr.lines().count(),
            1,
            "{word} 错误应只有一行：{stderr:?}"
        );
        assert!(
            stderr.starts_with("✗ "),
            "{word} 错误应以 ✗ 开头：{stderr:?}"
        );
        assert!(
            stderr.contains("ccms --help/-h 看用法"),
            "{word} 应提示旧式帮助：{stderr:?}"
        );
        assert!(
            !stderr.contains("Usage:") && !stderr.contains("Commands:"),
            "{word} 不得出现 clap 页面：{stderr:?}"
        );
    }
}

#[test]
fn unknown_commands_are_chinese_and_nonzero() {
    let ctx = Ctx::new("unknown");

    for args in [vec!["frobnicate"], vec!["--bogus"], vec!["-V"]] {
        let stderr = ctx.fail(&args);
        assert_eq!(stderr.lines().count(), 1, "错误应只有一行：{stderr:?}");
        assert!(stderr.starts_with("✗ "), "错误应以 ✗ 开头：{stderr:?}");
        assert!(
            stderr.contains("ccms --help/-h 看用法"),
            "错误应提示旧式帮助：{stderr:?}"
        );
    }

    let stderr = ctx.fail(&["--route", "frob"]);
    assert!(
        stderr.contains("未知操作: frob"),
        "旧式嵌套错误不对：{stderr:?}"
    );
    assert!(stderr.contains("--route/-r"));

    // 裸首词连嵌套参数也一律按未知命令处理。
    let stderr = ctx.fail(&["route", "frob"]);
    assert!(
        stderr.contains("未知命令: route"),
        "裸子命令必须先于嵌套解析被拒绝：{stderr:?}"
    );
}

#[test]
fn missing_arguments_use_legacy_usage() {
    let ctx = Ctx::new("missing");

    for (args, expected) in [
        (vec!["--key"], "用法: ccms --key/-k"),
        (vec!["--models"], "用法: ccms --models/-m"),
        (vec!["--test"], "用法: ccms --test/-t"),
        (vec!["--route", "--set"], "用法: ccms --route/-r --set"),
        (vec!["--route", "--unset"], "用法: ccms --route/-r --unset"),
        (vec!["--route", "--all"], "用法: ccms --route/-r --all"),
        (vec!["--stats", "--by"], "用法: ccms --stats/-u"),
    ] {
        let stderr = ctx.fail(&args);
        assert_eq!(stderr.lines().count(), 1, "错误应只有一行：{stderr:?}");
        assert!(
            stderr.contains(expected),
            "ccms {args:?} 应提示 {expected}，实际：{stderr:?}"
        );
    }
}

#[test]
fn corrupt_stats_and_vault_are_reported_as_errors() {
    let ctx = Ctx::new("corrupt");

    ctx.ok(&["--init"]);
    let stats = ctx.home.join("stats.json");
    std::fs::write(&stats, br#"{"version":2,"daily":{"#).unwrap();
    let stderr = ctx.fail(&["--stats", "--json"]);
    assert!(
        stderr.contains("stats") || stderr.contains("统计"),
        "stats 应显示解析错误：{stderr:?}"
    );
    assert_eq!(
        std::fs::read(&stats).unwrap(),
        br#"{"version":2,"daily":{"#,
        "stats 命令不得改写损坏文件"
    );

    std::fs::write(ctx.home.join("keys.enc"), b"not a vault").unwrap();
    let stderr = ctx.fail(&["--list"]);
    assert!(
        stderr.contains("密钥库"),
        "损坏密钥库不得伪装为空池：{stderr:?}"
    );
    let stderr = ctx.fail(&["--key", "kimi", "list"]);
    assert!(stderr.contains("密钥库"), "key list 也必须报错：{stderr:?}");
}

#[test]
fn plaintext_migration_failure_aborts_before_listening() {
    let ctx = Ctx::new("migrate-fail");
    ctx.ok(&["--init"]);

    let routes_path = ctx.home.join("routes.json");
    let mut routes: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&routes_path).unwrap()).unwrap();
    routes["listen"] = serde_json::json!("127.0.0.1:0");
    std::fs::write(&routes_path, serde_json::to_vec(&routes).unwrap()).unwrap();

    std::fs::write(
        ctx.home.join("keys.env"),
        "KIMI_API_KEYS=sk-plaintext-dummy\n",
    )
    .unwrap();
    // keys.enc 是目录时 rename 必然失败，构造一个不依赖 root/chmod 的迁移错误。
    std::fs::create_dir(ctx.home.join("keys.enc")).unwrap();

    let mut child = ctx
        .cmd(&["--router", "--run"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn router run");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("wait killed router");
            panic!(
                "明文迁移失败后 router 不得继续监听\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let output = child.wait_with_output().expect("output");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "迁移失败必须非零退出");
    assert!(
        stderr.contains("明文密钥迁移失败") && stderr.contains("keys.enc"),
        "错误应明确指出迁移失败原因：{stderr:?}"
    );
}

#[test]
fn missing_home_and_overrides_is_a_clear_error() {
    let output = Command::new(BIN)
        .arg("--version")
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .env_remove("CCMS_HOME")
        .env_remove("CLAUDE_SETTINGS")
        .output()
        .expect("spawn ccms");
    assert!(!output.status.success(), "没有 home 时不得回退到当前目录");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("主目录") && stderr.contains("CCMS_HOME"),
        "应给出明确的 home 配置错误：{stderr:?}"
    );
    assert!(
        !stderr.contains("./.config") && !stderr.contains("./.claude"),
        "错误信息不得暗示写入当前目录：{stderr:?}"
    );
}

#[test]
fn explicit_home_and_settings_work_without_home_env() {
    let ctx = Ctx::new("no-home-overrides");
    let output = ctx
        .cmd(&["--version"])
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .output()
        .expect("spawn ccms");
    assert!(
        output.status.success(),
        "CCMS_HOME 与 CLAUDE_SETTINGS 都显式设置时应可工作\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("0.2.1"));
}
