mod cli;
mod commands;
mod config;
mod fsutil;
mod http;
mod keystore;
mod paths;
mod proc;
mod router;
mod settings;
mod stats;
mod term;

use clap::Parser;

use cli::{Cli, Command};
use paths::Paths;

#[tokio::main]
async fn main() {
    // 旧版 `ccms --on` 之类的写法先翻译成子命令，再交给 clap 解析。
    let argv = cli::translate_legacy(std::env::args_os().collect());
    let cli = Cli::parse_from(argv);
    let paths = Paths::load();

    if let Err(err) = dispatch(cli.command, paths).await {
        eprintln!("✗ {err:#}");
        std::process::exit(1);
    }
}

async fn dispatch(command: Option<Command>, paths: Paths) -> anyhow::Result<()> {
    match command {
        None => commands::dashboard::run(&paths).await,
        Some(Command::Init { force }) => commands::init::run(&paths, force),
        Some(Command::On) => commands::mode::on(&paths).await,
        Some(Command::Off) => commands::mode::off(&paths),
        Some(Command::Status { json }) => commands::diag::status(&paths, json).await,
        Some(Command::List) => commands::diag::list(&paths),
        Some(Command::Key {
            provider,
            action,
            value,
        }) => commands::keys::run(&paths, &provider, action.as_deref(), value.as_deref()),
        Some(Command::Vault { cmd }) => {
            let cmd = cmd.unwrap_or(cli::VaultCmd::Status);
            commands::vault::run(&paths, cmd).await
        }
        Some(Command::Models { provider }) => commands::diag::models(&paths, &provider).await,
        Some(Command::Test { target }) => commands::diag::test(&paths, target.as_deref()).await,
        Some(Command::Route { cmd }) => commands::routes::run(&paths, cmd),
        Some(Command::Router { cmd }) => commands::router::run(&paths, cmd).await,
        Some(Command::Stats { arg, json, by }) => {
            commands::stats::run(&paths, arg.as_deref(), json, by.as_deref()).await
        }
        Some(Command::Doctor) => commands::diag::doctor(&paths).await,
        Some(Command::Version) => {
            cli::print_version();
            Ok(())
        }
    }
}
