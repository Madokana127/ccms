pub mod dashboard;
pub mod diag;
pub mod init;
pub mod keys;
pub mod mode;
pub mod router;
pub mod routes;
pub mod stats;
pub mod vault;

use crate::config::{tier_label, RoutesConfig, TIERS};
use crate::term::Style;

/// 统一的档位 → provider/model 渲染，CLI 各处共用。
pub fn print_routes(routes: &RoutesConfig, style: &Style) {
    for tier in TIERS {
        let (provider, model) = routes
            .routes
            .get(tier)
            .map(|route| (route.provider.as_str(), route.model.as_str()))
            .unwrap_or(("-", "-"));
        println!(
            "  {} {} {}",
            style.tier(&format!("{:<9}", tier_label(tier))),
            style.dim("→"),
            style.cyan(&format!("{provider}/{model}"))
        );
    }
}

pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 10 {
        return "***".to_string();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}
