use std::time::Duration;

use anyhow::{bail, Result};

use crate::config;
use crate::paths::Paths;
use crate::proc;
use crate::settings;
use crate::term::Style;

pub async fn on(paths: &Paths) -> Result<()> {
    config::ensure_defaults(paths)?;
    let style = Style::detect();
    let routes = config::load_routes(&paths.routes)?;
    let token = crate::token::ensure(paths)?;

    if !routes.routes.contains_key(&routes.default) {
        bail!(
            "默认档位 {} 没有路由，先执行: ccms --route/-r --set {} <provider> <model>",
            routes.default,
            routes.default
        );
    }

    if !proc::alive(&routes.listen).await {
        proc::spawn(paths)?;
        if !proc::wait_alive(&routes.listen, Duration::from_secs(8)).await {
            bail!(
                "router 启动失败，看 ccms --router --logs（{}）",
                paths.log.display()
            );
        }
    }

    let base = format!("http://{}", routes.listen);
    settings::enable(
        &paths.settings,
        &paths.settings_previous,
        &base,
        &routes.effort,
        &routes.default,
        &token,
    )?;

    println!("{}", style.ok(&format!("路由模式已启用 → {base}")));
    super::print_routes(&routes, &style);
    println!("{}", style.dim("新开一个 claude 会话生效"));
    Ok(())
}

pub fn off(paths: &Paths) -> Result<()> {
    let style = Style::detect();
    settings::disable(&paths.settings, &paths.settings_previous)?;
    println!(
        "{}",
        style.ok("已关闭路由，Claude Code 回到官方端点（router 进程可继续保留）")
    );
    Ok(())
}
