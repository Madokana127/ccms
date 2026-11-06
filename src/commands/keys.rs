use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;

use crate::config;
use crate::keystore::{self, Vault};
use crate::paths::Paths;
use crate::term::Style;

/// 在锁内重写池变量；只有一把 key 时也统一写池变量，并清掉单 key 变量。
fn write_pool(
    map: &mut BTreeMap<String, String>,
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

pub fn run(
    paths: &Paths,
    provider_name: &str,
    action: Option<&str>,
    value: Option<&str>,
    stdin: bool,
) -> Result<()> {
    config::ensure_defaults(paths)?;
    let style = Style::detect();
    let providers = config::load_providers(&paths.providers)?;
    let provider = config::require_provider(&providers, provider_name)?;

    let pool_var = provider
        .pool_var()
        .ok_or_else(|| anyhow!("{provider_name} 未配置 key_env，无法定位密钥池变量"))?;
    let single_var = provider.key_env.clone();
    let vault = Vault::new(paths);

    if stdin {
        if matches!(action, Some("list" | "clear" | "remove")) {
            bail!("--stdin 只能用于新增 key");
        }
        let keys = read_stdin_keys()?;
        // 读取与写回在同一把文件锁内完成，避免并发新增互相覆盖。
        let (added, total) = vault.update_with(|map| {
            let mut list = keystore::keys_for(map, provider);
            let mut added = Vec::new();
            for key in keys {
                if !list.iter().any(|current| current == &key) {
                    list.push(key.clone());
                    added.push(key);
                }
            }
            if !added.is_empty() {
                write_pool(map, &pool_var, single_var.as_deref(), &list);
            }
            Ok((added, list.len()))
        })?;
        if added.is_empty() {
            println!("{}", style.warn("这些 key 已在池中"));
            return Ok(());
        }
        let masks: Vec<String> = added.iter().map(|key| super::mask(key)).collect();
        println!(
            "{}",
            style.ok(&format!(
                "{provider_name} 密钥池 {total} 把（新增 {}: {}）",
                added.len(),
                masks.join(", ")
            ))
        );
        return Ok(());
    }

    match action {
        None | Some("list") => {
            let existing = keystore::keys_for(&vault.load()?, provider);
            if existing.is_empty() {
                println!("{}", style.warn(&format!("{provider_name} 的密钥池为空")));
                println!(
                    "{}",
                    style.dim(&format!("添加: ccms --key {provider_name} sk-xxx"))
                );
            } else {
                for (index, key) in existing.iter().enumerate() {
                    println!("  {}  {}", index + 1, super::mask(key));
                }
                println!(
                    "{}",
                    style.dim(&format!("共 {} 把 · 池变量 {pool_var}", existing.len()))
                );
            }
            Ok(())
        }
        Some("clear") => {
            vault.update_with(|map| {
                write_pool(map, &pool_var, single_var.as_deref(), &[]);
                Ok(())
            })?;
            println!("{}", style.ok(&format!("已清空 {provider_name} 的密钥池")));
            Ok(())
        }
        Some("remove") => {
            let index: usize = value
                .ok_or_else(|| anyhow!("用法: ccms --key/-k {provider_name} remove <序号>"))?
                .parse()
                .map_err(|_| anyhow!("序号必须是数字"))?;
            let remaining = vault.update_with(|map| -> Result<usize> {
                let mut list = keystore::keys_for(map, provider);
                if index == 0 || index > list.len() {
                    bail!("序号超出范围（1..={}）", list.len());
                }
                list.remove(index - 1);
                let remaining = list.len();
                write_pool(map, &pool_var, single_var.as_deref(), &list);
                Ok(remaining)
            })?;
            println!(
                "{}",
                style.ok(&format!("已移除第 {index} 把，剩余 {remaining} 把"))
            );
            Ok(())
        }
        Some(other) => {
            let key = if other == "add" {
                value.ok_or_else(|| anyhow!("用法: ccms --key/-k {provider_name} add <sk-...>"))?
            } else {
                other
            };
            let key = key.trim();
            if key.is_empty() {
                bail!("key 不能为空");
            }
            let (added, total) = vault.update_with(|map| {
                let mut list = keystore::keys_for(map, provider);
                if list.iter().any(|current| current == key) {
                    return Ok((false, list.len()));
                }
                list.push(key.to_string());
                let total = list.len();
                write_pool(map, &pool_var, single_var.as_deref(), &list);
                Ok((true, total))
            })?;
            if !added {
                println!("{}", style.warn("该 key 已在池中"));
                return Ok(());
            }
            println!(
                "{}",
                style.ok(&format!(
                    "{provider_name} 密钥池 {total} 把（新增 {}）",
                    super::mask(key)
                ))
            );
            Ok(())
        }
    }
}

fn read_stdin_keys() -> Result<Vec<String>> {
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .context("读取 stdin 失败")?;
    let keys: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .collect();
    if keys.is_empty() {
        bail!("stdin 为空：请通过管道传入至少一把 key");
    }
    Ok(keys)
}
