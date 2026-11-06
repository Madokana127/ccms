use anyhow::{bail, Result};

use crate::cli::RouteCmd;
use crate::config::{self, Route, TIERS};
use crate::paths::Paths;
use crate::term::Style;

pub fn run(paths: &Paths, cmd: Option<RouteCmd>) -> Result<()> {
    config::ensure_defaults(paths)?;
    let style = Style::detect();
    let mut cfg = config::load_routes(&paths.routes)?;
    let providers = config::load_providers(&paths.providers)?;

    match cmd.unwrap_or(RouteCmd::List) {
        RouteCmd::List => {
            println!("{}", style.section("路由表"));
            super::print_routes(&cfg, &style);
            println!(
                "  {}",
                style.dim(&format!(
                    "default = {}   effort = {}   listen = {}",
                    cfg.default, cfg.effort, cfg.listen
                ))
            );
            return Ok(());
        }
        RouteCmd::Set {
            tier,
            provider,
            model,
        } => {
            let tier = tier.to_ascii_lowercase();
            if !config::is_tier(&tier) {
                bail!("未知档位 {tier}（可用: {}）", TIERS.join(" / "));
            }
            config::require_provider(&providers, &provider)?;
            cfg.routes.insert(tier.clone(), Route { provider, model });
            config::save_routes(&paths.routes, &cfg)?;
            println!("{}", style.ok(&format!("{tier} 已更新")));
        }
        RouteCmd::Unset { tier } => {
            let tier = tier.to_ascii_lowercase();
            if cfg.routes.remove(&tier).is_none() {
                println!("{}", style.warn(&format!("{tier} 本来就没有路由")));
                return Ok(());
            }
            config::save_routes(&paths.routes, &cfg)?;
            println!("{}", style.ok(&format!("已删除路由 {tier}")));
        }
        RouteCmd::All { provider, model } => {
            let target = config::require_provider(&providers, &provider)?;
            let model = model.unwrap_or_else(|| target.default_model());
            if model.is_empty() {
                bail!("{provider} 没有 default_model，请显式指定模型");
            }
            for tier in TIERS {
                cfg.routes.insert(
                    tier.to_string(),
                    Route {
                        provider: provider.clone(),
                        model: model.clone(),
                    },
                );
            }
            config::save_routes(&paths.routes, &cfg)?;
            println!(
                "{}",
                style.ok(&format!("五个档位已统一到 {provider}/{model}"))
            );
        }
    }

    super::print_routes(&cfg, &style);
    println!("{}", style.dim("router 会热加载路由，无需重启。"));
    Ok(())
}
