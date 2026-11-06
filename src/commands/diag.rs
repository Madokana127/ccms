use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::config;
use crate::fsutil;
use crate::keystore::{self, Vault};
use crate::paths::Paths;
use crate::proc;
use crate::settings;
use crate::term::Style;

pub async fn status(paths: &Paths, json_output: bool) -> Result<()> {
    config::ensure_defaults(paths)?;
    let style = Style::detect();
    let routes = config::load_routes(&paths.routes)?;
    let providers = config::load_providers(&paths.providers)?;
    let settings = settings::read(&paths.settings)?;
    let vault = Vault::new(paths);
    let vault_status = vault.status();
    let alive = proc::alive(&routes.listen).await;
    let claude = which("claude");
    let claude_version = claude.as_ref().and_then(|path| {
        Command::new(path)
            .arg("--version")
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });

    if json_output {
        let payload = json!({
            "version": env!("CARGO_PKG_VERSION"),
            "listen": routes.listen,
            "default": routes.default,
            "effort": routes.effort,
            "router_alive": alive,
            "routes": routes.routes,
            "providers": providers.keys().cloned().collect::<Vec<_>>(),
            "vault": {
                "encrypted": vault_status.encrypted,
                "mode": vault_status.mode,
                "entries": vault_status.entries,
                "decryptable": vault_status.decryptable,
            },
            "settings": paths.settings.display().to_string(),
            "base_url": settings::base_url(&settings),
            "claude": {
                "path": claude.as_ref().map(|p| p.display().to_string()),
                "version": claude_version,
            },
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("{}", style.section("Claude Code"));
    match (&claude, &claude_version) {
        (Some(path), Some(version)) => {
            println!("  路径  {}", path.display());
            println!("  版本  {version}");
        }
        (Some(path), None) => println!("  路径  {}", path.display()),
        (None, _) => println!("  {}", style.warn("claude 不在 PATH")),
    }

    println!("{}", style.section("Router"));
    if alive {
        println!(
            "  {}",
            style.ok(&format!("运行中 http://{}", routes.listen))
        );
    } else {
        println!(
            "  {}",
            style.warn(&format!("未运行 http://{}", routes.listen))
        );
    }

    println!("{}", style.section("settings.json"));
    println!("  {}", paths.settings.display());
    match settings.get("env").and_then(|env| env.as_object()) {
        Some(env) if !env.is_empty() => {
            for (key, value) in env {
                let shown = if key.contains("TOKEN") || key.contains("KEY") {
                    "***".to_string()
                } else {
                    value.as_str().unwrap_or("").to_string()
                };
                println!("  {key}={shown}");
            }
        }
        _ => println!("  (无 env 段)"),
    }

    println!("{}", style.section("密钥库"));
    if vault_status.encrypted {
        println!(
            "  {} {} · {} 条目{}",
            paths.keys_enc.display(),
            vault_status.cipher,
            vault_status.entries,
            if vault_status.decryptable {
                ""
            } else {
                " · 解密失败"
            }
        );
    } else {
        println!(
            "  {}",
            style.warn("明文 keys.env（ccms --vault --seal 可迁移到加密存储）")
        );
    }
    Ok(())
}

pub fn list(paths: &Paths) -> Result<()> {
    config::ensure_defaults(paths)?;
    let providers = config::load_providers(&paths.providers)?;
    let env = Vault::new(paths).load()?;

    println!(
        "{:<12}{:<20}{:<46}{:<7}DEFAULT MODEL",
        "PROVIDER", "NAME", "BASE_URL", "KEYS"
    );
    for (name, provider) in &providers {
        let count = keystore::keys_for(&env, provider).len();
        let keys = if count > 0 {
            format!("✓ {count}")
        } else {
            "✗".to_string()
        };
        println!(
            "{:<12}{:<20}{:<46}{:<7}{}",
            name,
            provider.display(name),
            provider.base_url,
            keys,
            provider.default_model()
        );
    }
    Ok(())
}

pub async fn models(paths: &Paths, provider_name: &str) -> Result<()> {
    config::ensure_defaults(paths)?;
    let providers = config::load_providers(&paths.providers)?;
    let provider = config::require_provider(&providers, provider_name)?;
    let env = Vault::new(paths).load()?;
    let keys = keystore::keys_for(&env, provider);
    let Some(key) = keys.first() else {
        bail!("{provider_name} 没有可用 key");
    };

    let url = format!("{}/v1/models", provider.base_url.trim_end_matches('/'));
    let client = crate::http::upstream(Some(Duration::from_secs(25)), None)?;
    let request = client.get(url).header("anthropic-version", "2023-06-01");
    let request = if provider.uses_x_api_key() {
        request.header("x-api-key", key)
    } else {
        request.bearer_auth(key)
    };

    let response = request.send().await?;
    let text = response.text().await?;
    match serde_json::from_str::<Value>(&text) {
        Ok(value) if value.get("data").and_then(|d| d.as_array()).is_some() => {
            for model in value["data"].as_array().expect("已检查") {
                println!(
                    "{}\t{}",
                    model["id"].as_str().unwrap_or(""),
                    model["display_name"].as_str().unwrap_or("")
                );
            }
        }
        _ => {
            println!("该端点没有标准 /v1/models，原始返回：");
            println!("{}", snippet(&text, 800));
        }
    }
    Ok(())
}

pub async fn test(paths: &Paths, target: Option<&str>) -> Result<()> {
    config::ensure_defaults(paths)?;
    let style = Style::detect();
    let target = target.ok_or_else(|| anyhow!("用法: ccms --test/-t <provider|router|all>"))?;

    match target {
        "router" => test_router(paths, &style).await,
        "all" => {
            let providers = config::load_providers(&paths.providers)?;
            let mut failures = 0;
            for name in providers.keys() {
                if test_provider(paths, name, &style).await.is_err() {
                    failures += 1;
                }
            }
            if failures > 0 {
                bail!("{failures} 个 provider 未通过");
            }
            Ok(())
        }
        name => test_provider(paths, name, &style).await,
    }
}

async fn test_provider(paths: &Paths, name: &str, style: &Style) -> Result<()> {
    let providers = config::load_providers(&paths.providers)?;
    let provider = config::require_provider(&providers, name)?;
    let env = Vault::new(paths).load()?;
    let keys = keystore::keys_for(&env, provider);
    let Some(key) = keys.first() else {
        bail!("还没填 {name} 的 key");
    };
    let model = provider.default_model();
    if model.is_empty() {
        bail!("{name} 没有配置模型");
    }

    let url = format!("{}/v1/messages", provider.base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "say ok" }],
    });
    let request = crate::http::upstream(Some(Duration::from_secs(60)), None)?
        .post(&url)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");
    let request = if provider.uses_x_api_key() {
        request.header("x-api-key", key)
    } else {
        request.bearer_auth(key)
    };

    println!("{}", style.section(&format!("POST {url}  model={model}")));
    let response = request.body(serde_json::to_vec(&body)?).send().await?;
    let status = response.status().as_u16();
    let text = response.text().await?;
    println!("HTTP {status}");
    println!("{}", snippet(&text, 800));
    if (200..300).contains(&status) {
        println!("{}", style.ok("通道可用"));
        Ok(())
    } else {
        bail!("请求失败（401=key/鉴权头不对，404=model 或路径不对）");
    }
}

async fn test_router(paths: &Paths, style: &Style) -> Result<()> {
    let routes = config::load_routes(&paths.routes)?;
    if !proc::alive(&routes.listen).await {
        bail!("router 未运行（ccms --router --start）");
    }
    let url = format!("http://{}/v1/messages", routes.listen);
    let token = crate::token::ensure(paths)?;
    let body = json!({
        "model": format!("ccms-{}", routes.default),
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "say ok" }],
    });

    println!("{}", style.section(&format!("POST {url}")));
    let response = crate::http::local_with_timeout(Duration::from_secs(60))?
        .post(&url)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .header("x-api-key", token)
        .body(serde_json::to_vec(&body)?)
        .send()
        .await?;
    let status = response.status().as_u16();
    let text = response.text().await?;
    println!("HTTP {status}");
    println!("{}", snippet(&text, 800));
    if status == 200 {
        println!("{}", style.ok("路由链路可用"));
        Ok(())
    } else {
        bail!("路由链路失败，看 ccms --router --logs");
    }
}

pub async fn doctor(paths: &Paths) -> Result<()> {
    let style = Style::detect();
    config::ensure_defaults(paths)?;

    println!("{}", style.section("环境"));
    println!(
        "  ccms     {} ({} {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  配置目录 {}", paths.home.display());
    match which("claude") {
        Some(path) => println!("  claude   {}", path.display()),
        None => println!("  claude   {}", style.warn("不在 PATH")),
    }

    println!("{}", style.section("配置文件"));
    for (label, path) in [
        ("routes.json", &paths.routes),
        ("providers.json", &paths.providers),
        ("settings.json", &paths.settings),
    ] {
        if path.is_file() {
            println!(
                "  {:<16}{}  {}",
                label,
                fsutil::mode_string(path),
                path.display()
            );
        } else {
            println!("  {:<16}{}", label, style.warn("缺失"));
        }
    }
    if let Ok(text) = std::fs::read_to_string(&paths.settings) {
        if !text.trim().is_empty() && serde_json::from_str::<Value>(&text).is_err() {
            println!("  {}", style.fail("settings.json 不是合法 JSON"));
        }
    }

    println!("{}", style.section("密钥库"));
    let vault = Vault::new(paths);
    let status = vault.status();
    if status.encrypted {
        println!(
            "  {} · {} · {} 条目{}",
            status.cipher,
            status.mode,
            status.entries,
            if status.decryptable {
                ""
            } else {
                " · 解密失败"
            }
        );
        if let Some(err) = &status.error {
            println!("  {}", style.fail(err));
        }
    } else {
        println!(
            "  {}",
            style.warn("明文 keys.env（ccms --vault --seal 加密）")
        );
    }

    println!("{}", style.section("密钥"));
    let providers = config::load_providers(&paths.providers)?;
    match vault.load() {
        Ok(env) => {
            for (name, provider) in &providers {
                let count = keystore::keys_for(&env, provider).len();
                if count > 0 {
                    println!("  {}", style.ok(&format!("{name}（{count} 把）")));
                } else {
                    println!("  {}", style.warn(&format!("{name} 未填")));
                }
            }
        }
        Err(err) => println!("  {}", style.fail(&format!("读取密钥失败: {err:#}"))),
    }

    println!("{}", style.section("Router"));
    let routes = config::load_routes(&paths.routes)?;
    if proc::alive(&routes.listen).await {
        println!(
            "  {}",
            style.ok(&format!("运行中 http://{}", routes.listen))
        );
        super::print_routes(&routes, &style);
    } else {
        println!("  {}", style.warn("未运行（ccms --router --start）"));
    }

    Ok(())
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<&str> = if cfg!(windows) {
        vec![".exe", ".cmd", ".bat", ""]
    } else {
        vec![""]
    };
    for dir in std::env::split_paths(&path) {
        for extension in &extensions {
            let candidate = dir.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn snippet(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}…")
}
