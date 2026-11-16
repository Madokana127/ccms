use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::fsutil;
use crate::paths::Paths;

/// 档位是 Claude Code 的硬契约，新增档位需要同步 Claude 侧环境变量。
pub const TIERS: [&str; 5] = ["fable", "opus", "sonnet", "haiku", "subagent"];

/// 默认配置内嵌进二进制。
pub const DEFAULT_CONFIG_TOML: &str = include_str!("../config/config.toml");

#[cfg_attr(not(test), allow(dead_code))]
pub fn is_tier(tier: &str) -> bool {
    TIERS.contains(&tier)
}

fn default_listen() -> String {
    "127.0.0.1:8788".into()
}
fn default_tier() -> String {
    "sonnet".into()
}
fn default_effort() -> String {
    "high".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub provider: String,
    pub model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Retry {
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    #[serde(default = "default_backoff")]
    pub backoff_ms: u64,
    #[serde(default = "default_response_timeout", alias = "timeout_ms")]
    pub response_timeout_ms: u64,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_retry_on")]
    pub retry_on: Vec<u16>,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: default_attempts(),
            backoff_ms: default_backoff(),
            response_timeout_ms: default_response_timeout(),
            connect_timeout_ms: default_connect_timeout(),
            retry_on: default_retry_on(),
        }
    }
}

fn default_attempts() -> u32 {
    3
}
fn default_backoff() -> u64 {
    400
}
fn default_response_timeout() -> u64 {
    120_000
}
fn default_connect_timeout() -> u64 {
    15_000
}
fn default_retry_on() -> Vec<u16> {
    vec![401, 403, 408, 429, 500, 502, 503, 504]
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoutesConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_tier")]
    pub default: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default)]
    pub retry: Retry,
    #[serde(default)]
    pub routes: BTreeMap<String, Route>,
}

impl Default for RoutesConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            default: default_tier(),
            effort: default_effort(),
            retry: Retry::default(),
            routes: BTreeMap::new(),
        }
    }
}

pub type Providers = BTreeMap<String, Provider>;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Provider {
    #[serde(default)]
    pub label: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub key_env: Option<String>,
    #[serde(default)]
    pub key_env_pool: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

impl Provider {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn default_model(&self) -> String {
        self.default_model
            .clone()
            .or_else(|| self.model.clone())
            .or_else(|| self.models.first().cloned())
            .unwrap_or_default()
    }

    pub fn pool_var(&self) -> Option<String> {
        if let Some(pool) = &self.key_env_pool {
            return Some(pool.clone());
        }
        self.key_env.as_ref().map(|var| {
            var.strip_suffix("_KEY")
                .map(|stem| format!("{stem}_KEYS"))
                .unwrap_or_else(|| format!("{var}_KEYS"))
        })
    }

    pub fn uses_x_api_key(&self) -> bool {
        matches!(self.auth.as_deref(), Some("x-api-key") | Some("x_api_key"))
    }
}

/// 统一 TOML 配置结构。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AppConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_tier")]
    pub default: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default)]
    pub retry: Retry,
    #[serde(default)]
    pub routes: BTreeMap<String, Route>,
    #[serde(default)]
    pub providers: Providers,
}

impl AppConfig {
    pub fn split(&self) -> (RoutesConfig, Providers) {
        (
            RoutesConfig {
                listen: self.listen.clone(),
                default: self.default.clone(),
                effort: self.effort.clone(),
                retry: self.retry.clone(),
                routes: self.routes.clone(),
            },
            self.providers.clone(),
        )
    }
}

/// 读取生效配置：优先 `config.toml`，若不存在则兼容读取旧版 `routes.json` + `providers.json`。
pub fn load_effective(paths: &Paths) -> Result<(RoutesConfig, Providers)> {
    if paths.config.is_file() {
        let text = fs::read_to_string(&paths.config)
            .with_context(|| format!("读取 {}", paths.config.display()))?;
        let cfg: AppConfig = toml::from_str(&text)
            .with_context(|| format!("{} 不是合法的 TOML 配置", paths.config.display()))?;
        return Ok(cfg.split());
    }
    let routes = load_routes(&paths.routes)?;
    let providers = load_providers(&paths.providers)?;
    Ok((routes, providers))
}

pub fn load_routes(path: &Path) -> Result<RoutesConfig> {
    let text = fs::read_to_string(path).with_context(|| format!("读取 {}", path.display()))?;
    let cfg: RoutesConfig = serde_json::from_str(&text)
        .with_context(|| format!("{} 不是合法的路由配置", path.display()))?;
    Ok(cfg)
}

pub fn load_providers(path: &Path) -> Result<Providers> {
    let text = fs::read_to_string(path).with_context(|| format!("读取 {}", path.display()))?;
    let providers: Providers = serde_json::from_str(&text)
        .with_context(|| format!("{} 不是合法的 provider 配置", path.display()))?;
    Ok(providers)
}

pub fn require_provider<'a>(providers: &'a Providers, name: &str) -> Result<&'a Provider> {
    providers.get(name).ok_or_else(|| {
        anyhow::anyhow!(
            "未知 provider: {name}（可用: {}）",
            providers.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })
}

/// 若 `config.toml` 与旧版 JSON 都不存在，写入带注释的默认 `config.toml`。
pub fn ensure_defaults(paths: &Paths) -> Result<()> {
    paths.ensure().context("创建 ALKANET_HOME 失败")?;
    if !paths.config.exists() && !paths.routes.exists() {
        fsutil::write_atomic(&paths.config, DEFAULT_CONFIG_TOML.as_bytes(), 0o600)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_toml_parses() {
        let cfg: AppConfig = toml::from_str(DEFAULT_CONFIG_TOML).expect("config.toml");
        assert!(is_tier(&cfg.default));
        for (tier, route) in &cfg.routes {
            assert!(is_tier(tier), "未知档位 {tier}");
            assert!(
                cfg.providers.contains_key(&route.provider),
                "{} 未定义",
                route.provider
            );
            assert!(!route.model.is_empty());
        }
    }

    #[test]
    fn legacy_provider_fields_still_parse() {
        let raw = r#"{"kimi":{"base_url":"https://x","key_env":"KIMI_API_KEY","model":"kimi-k3"}}"#;
        let providers: Providers = serde_json::from_str(raw).unwrap();
        let kimi = &providers["kimi"];
        assert_eq!(kimi.default_model(), "kimi-k3");
        assert_eq!(kimi.pool_var().as_deref(), Some("KIMI_API_KEYS"));
    }

    #[test]
    fn legacy_timeout_key_is_accepted() {
        let raw = r#"{"listen":"127.0.0.1:1","timeout_ms":9000}"#;
        let retry: Retry = serde_json::from_str(raw).unwrap();
        assert_eq!(retry.response_timeout_ms, 9000);
    }
}
