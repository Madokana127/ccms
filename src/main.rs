use std::env;
use std::io::Read;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;

use crate::keystore::Vault;
use crate::paths::Paths;

mod config;
mod fsutil;
mod http;
mod keystore;
mod paths;
mod router;
mod stats;
mod token;

#[tokio::main]
async fn main() {
    let paths = match Paths::load() {
        Ok(paths) => paths,
        Err(err) => {
            eprintln!("✗ {err:#}");
            std::process::exit(1);
        }
    };

    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(err) = dispatch(&args, paths).await {
        eprintln!("✗ {err:#}");
        std::process::exit(1);
    }
}

async fn dispatch(args: &[String], paths: Paths) -> Result<()> {
    match args.first().map(String::as_str) {
        None | Some("--version") | Some("-v") => {
            println!("alkanetd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") => {
            println!(
                "alkanetd {} — Alkanet daemon & encrypted vault core",
                env!("CARGO_PKG_VERSION")
            );
            println!("用法: alkanetd <run|vault ...>");
            Ok(())
        }
        Some("run") | Some("--run") => router::run(paths).await,
        Some("vault") => dispatch_vault(&args[1..], &paths),
        Some(other) => bail!("未知内部命令: {other}"),
    }
}

fn dispatch_vault(args: &[String], paths: &Paths) -> Result<()> {
    let vault = Vault::new(paths);
    match args.first().map(String::as_str) {
        Some("status") => {
            let st = vault.status();
            let payload = json!({
                "encrypted": st.encrypted,
                "cipher": st.cipher,
                "mode": st.mode,
                "keyfile": st.keyfile.map(|p| p.display().to_string()),
                "keyfile_mode": st.keyfile_mode,
                "entries": st.entries,
                "decryptable": st.decryptable,
                "updated": st.updated,
                "error": st.error,
            });
            println!("{}", serde_json::to_string(&payload)?);
            Ok(())
        }
        Some("seal") => {
            let (imported, entries) = vault.seal()?;
            println!(
                "{}",
                serde_json::to_string(&json!({ "imported": imported, "entries": entries }))?
            );
            Ok(())
        }
        Some("rekey") => {
            let (mode, entries) = vault.rekey()?;
            println!(
                "{}",
                serde_json::to_string(&json!({ "mode": mode, "entries": entries }))?
            );
            Ok(())
        }
        Some("list") => {
            let provider = args.get(1).ok_or_else(|| anyhow!("缺少 provider"))?;
            let (_, providers) = load_effective_or_defaults(paths)?;
            let p = config::require_provider(&providers, provider)?;
            let map = vault.load()?;
            let keys = keystore::keys_for(&map, p);
            println!("{}", serde_json::to_string(&keys)?);
            Ok(())
        }
        Some("counts") => {
            let (_, providers) = load_effective_or_defaults(paths)?;
            let map = vault.load()?;
            let mut counts = serde_json::Map::new();
            for (name, p) in &providers {
                counts.insert(name.clone(), json!(keystore::keys_for(&map, p).len()));
            }
            println!("{}", serde_json::to_string(&counts)?);
            Ok(())
        }
        Some("add") => {
            let provider = args.get(1).ok_or_else(|| anyhow!("缺少 provider"))?;
            let (_, providers) = load_effective_or_defaults(paths)?;
            let p = config::require_provider(&providers, provider)?;
            let pool_var = p
                .pool_var()
                .ok_or_else(|| anyhow!("{provider} 未配置 key_env，无法定位密钥池变量"))?;
            let single_var = p.key_env.clone();

            let incoming = if args.get(2).map(String::as_str) == Some("--stdin") {
                read_stdin_keys()?
            } else {
                let raw = args.get(2).ok_or_else(|| anyhow!("缺少 key"))?;
                let k = raw.trim().to_string();
                if k.is_empty() {
                    bail!("key 不能为空");
                }
                vec![k]
            };

            let (added, total) = vault.update_with(|map| {
                let mut list = keystore::keys_for(map, p);
                let mut added = Vec::new();
                for key in incoming {
                    if !list.iter().any(|c| c == &key) {
                        list.push(key.clone());
                        added.push(key);
                    }
                }
                if !added.is_empty() {
                    write_pool(map, &pool_var, single_var.as_deref(), &list);
                }
                Ok((added.len(), list.len()))
            })?;
            println!(
                "{}",
                serde_json::to_string(&json!({ "added": added, "total": total }))?
            );
            Ok(())
        }
        Some("rm") => {
            let provider = args.get(1).ok_or_else(|| anyhow!("缺少 provider"))?;
            let idx_str = args.get(2).ok_or_else(|| anyhow!("缺少序号"))?;
            let index: usize = idx_str.parse().map_err(|_| anyhow!("序号必须是数字"))?;
            let (_, providers) = load_effective_or_defaults(paths)?;
            let p = config::require_provider(&providers, provider)?;
            let pool_var = p
                .pool_var()
                .ok_or_else(|| anyhow!("{provider} 未配置 key_env，无法定位密钥池变量"))?;
            let single_var = p.key_env.clone();

            let remaining = vault.update_with(|map| {
                let mut list = keystore::keys_for(map, p);
                if index == 0 || index > list.len() {
                    bail!("序号超出范围（1..={}）", list.len());
                }
                list.remove(index - 1);
                let rem = list.len();
                write_pool(map, &pool_var, single_var.as_deref(), &list);
                Ok(rem)
            })?;
            println!(
                "{}",
                serde_json::to_string(&json!({ "remaining": remaining }))?
            );
            Ok(())
        }
        Some("clear") => {
            let provider = args.get(1).ok_or_else(|| anyhow!("缺少 provider"))?;
            let (_, providers) = load_effective_or_defaults(paths)?;
            let p = config::require_provider(&providers, provider)?;
            let pool_var = p
                .pool_var()
                .ok_or_else(|| anyhow!("{provider} 未配置 key_env，无法定位密钥池变量"))?;
            let single_var = p.key_env.clone();

            vault.update_with(|map| {
                write_pool(map, &pool_var, single_var.as_deref(), &[]);
                Ok(())
            })?;
            println!("{}", serde_json::to_string(&json!({ "cleared": true }))?);
            Ok(())
        }
        Some(other) => bail!("未知 vault 操作: {other}"),
        None => bail!("缺少 vault 子命令"),
    }
}

fn load_effective_or_defaults(paths: &Paths) -> Result<(config::RoutesConfig, config::Providers)> {
    config::ensure_defaults(paths)?;
    config::load_effective(paths)
}

fn write_pool(
    map: &mut std::collections::BTreeMap<String, String>,
    pool_var: &str,
    single_var: Option<&str>,
    list: &[String],
) {
    if list.is_empty() {
        map.remove(pool_var);
    } else {
        map.insert(pool_var.to_string(), list.join(","));
    }
    if let Some(single) = single_var {
        map.remove(single);
    }
}

fn read_stdin_keys() -> Result<Vec<String>> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .context("读取 stdin 失败")?;
    let keys: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect();
    if keys.is_empty() {
        bail!("stdin 为空：请通过管道传入至少一把 key");
    }
    Ok(keys)
}
