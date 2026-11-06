use anyhow::Result;

use crate::config;
use crate::keystore::{self, Vault};
use crate::paths::Paths;
use crate::proc;
use crate::settings;
use crate::term::Style;

pub async fn run(paths: &Paths) -> Result<()> {
    config::ensure_defaults(paths)?;
    let style = Style::detect();
    let routes = config::load_routes(&paths.routes)?;
    let providers = config::load_providers(&paths.providers)?;
    let settings = settings::read(&paths.settings)?;

    let local = format!("http://{}", routes.listen);
    let mode = match settings::base_url(&settings).as_deref() {
        Some(url) if url.trim_end_matches('/') == local.trim_end_matches('/') => {
            if proc::alive(&routes.listen).await {
                "Router".to_string()
            } else {
                "Router (stopped)".to_string()
            }
        }
        Some(_) => "Claude (direct)".to_string(),
        None => "Claude".to_string(),
    };

    println!("{} {}", style.bold("Mode"), mode);
    println!(
        "{} {} {}",
        style.bold("Default"),
        style.tier(&config::tier_label(&routes.default)),
        routes.effort
    );
    println!("{}", style.bold("Route List"));
    super::print_routes(&routes, &style);

    println!("{}", style.bold("API Key"));
    let vault = Vault::new(paths);
    match vault.load() {
        Ok(env) => {
            for (name, provider) in &providers {
                let count = keystore::keys_for(&env, provider).len();
                if count > 0 {
                    println!("    {} {} ({count})", style.green("✓"), name);
                } else {
                    println!("    {} {}", style.red("✗"), name);
                }
            }
        }
        Err(err) => println!("    {} 密钥库不可用: {err:#}", style.yellow("!")),
    }

    Ok(())
}
