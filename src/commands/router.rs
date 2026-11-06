use std::time::Duration;

use anyhow::{bail, Result};

use crate::cli::RouterCmd;
use crate::config;
use crate::paths::Paths;
use crate::proc;
use crate::term::Style;

pub async fn run(paths: &Paths, cmd: Option<RouterCmd>) -> Result<()> {
    config::ensure_defaults(paths)?;
    let style = Style::detect();
    let cfg = config::load_routes(&paths.routes)?;
    let listen = cfg.listen.clone();

    match cmd.unwrap_or(RouterCmd::Status) {
        RouterCmd::Run => crate::router::run(paths.clone()).await,
        RouterCmd::Start => start(paths, &style, &listen).await,
        RouterCmd::Stop => {
            if proc::stop(paths, &listen).await? {
                println!("{}", style.ok("router 已停止"));
            } else {
                println!("{}", style.warn("router 未在运行"));
            }
            Ok(())
        }
        RouterCmd::Restart => {
            proc::stop(paths, &listen).await?;
            start(paths, &style, &listen).await
        }
        RouterCmd::Ensure => {
            if !proc::alive(&listen).await {
                proc::spawn(paths)?;
                proc::wait_alive(&listen, Duration::from_secs(5)).await;
            }
            Ok(())
        }
        RouterCmd::Status => {
            println!("{}", style.section("ccms router"));
            match proc::health(&listen).await {
                Some(health) => {
                    println!("  {}", style.ok(&format!("运行中 http://{listen}")));
                    println!("  uptime   {} s", health["uptime_s"]);
                    println!("  version  {}", health["version"]);
                    println!("  default  {}", health["default"]);
                    println!("  路由表：");
                    if let Some(routes) = health["routes"].as_object() {
                        for (tier, route) in routes {
                            println!(
                                "    {} → {}/{}",
                                style.tier(&format!("{:<9}", config::tier_label(tier))),
                                route["provider"].as_str().unwrap_or("-"),
                                route["model"].as_str().unwrap_or("-")
                            );
                        }
                    }
                    if let Some(keys) = health["keys"].as_object() {
                        let summary: Vec<String> = keys
                            .iter()
                            .map(|(name, count)| format!("{name}={count}"))
                            .collect();
                        println!("  key      {}", summary.join("  "));
                    }
                }
                None => println!("  {}", style.warn("未运行（ccms router start）")),
            }
            Ok(())
        }
        RouterCmd::Logs { n } => {
            let text = proc::tail(&paths.log, n.unwrap_or(40));
            if text.is_empty() {
                println!("(暂无日志)");
            } else {
                println!("{text}");
            }
            Ok(())
        }
    }
}

async fn start(paths: &Paths, style: &Style, listen: &str) -> Result<()> {
    if proc::alive(listen).await {
        println!("{}", style.ok(&format!("router 已在运行 http://{listen}")));
        return Ok(());
    }
    proc::spawn(paths)?;
    if proc::wait_alive(listen, Duration::from_secs(8)).await {
        println!("{}", style.ok(&format!("router 已启动 http://{listen}")));
        Ok(())
    } else {
        bail!("router 启动失败，日志: {}", paths.log.display())
    }
}
