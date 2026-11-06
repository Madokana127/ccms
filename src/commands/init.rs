use anyhow::Result;

use crate::config;
use crate::paths::Paths;
use crate::term::Style;

pub fn run(paths: &Paths, force: bool) -> Result<()> {
    let style = Style::detect();
    paths.ensure().ok();

    if force {
        config::reset_defaults(paths, "routes")?;
        config::reset_defaults(paths, "providers")?;
    }
    config::ensure_defaults(paths)?;

    println!("{}", style.section("配置目录"));
    println!("  {}", paths.home.display());
    println!("  {:<16}{}", "routes.json", paths.routes.display());
    println!("  {:<16}{}", "providers.json", paths.providers.display());
    println!("  {:<16}{}", "settings.json", paths.settings.display());
    if force {
        println!("{}", style.ok("已重置为内置默认配置"));
    } else {
        println!(
            "{}",
            style.dim("已有文件不会被覆盖；--force 重置为内置默认。")
        );
    }
    println!(
        "{}",
        style.dim("下一步: ccms --key <provider> sk-...  →  ccms --on  →  claude")
    );
    Ok(())
}
