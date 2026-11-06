use anyhow::{anyhow, bail, Result};

use crate::config;
use crate::keystore::{self, Vault};
use crate::paths::Paths;
use crate::term::Style;

pub fn run(
    paths: &Paths,
    provider_name: &str,
    action: Option<&str>,
    value: Option<&str>,
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
    let env = vault.load_or_empty();
    let existing = keystore::keys_for(&env, provider);

    let write_pool = |list: &[String]| -> Result<()> {
        let joined = list.join(",");
        let pool_var = pool_var.clone();
        let single_var = single_var.clone();
        vault.update(move |map| {
            if joined.is_empty() {
                map.remove(&pool_var);
            } else {
                map.insert(pool_var.clone(), joined);
            }
            if let Some(single) = single_var {
                map.remove(&single);
            }
        })
    };

    match action {
        None | Some("list") => {
            if existing.is_empty() {
                println!("{}", style.warn(&format!("{provider_name} 的密钥池为空")));
                println!(
                    "{}",
                    style.dim(&format!("添加: ccms key {provider_name} sk-xxx"))
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
            write_pool(&[])?;
            println!("{}", style.ok(&format!("已清空 {provider_name} 的密钥池")));
            Ok(())
        }
        Some("remove") => {
            let index: usize = value
                .ok_or_else(|| anyhow!("用法: ccms key {provider_name} remove <序号>"))?
                .parse()
                .map_err(|_| anyhow!("序号必须是数字"))?;
            if index == 0 || index > existing.len() {
                bail!("序号超出范围（1..={}）", existing.len());
            }
            let mut list = existing.clone();
            list.remove(index - 1);
            write_pool(&list)?;
            println!(
                "{}",
                style.ok(&format!("已移除第 {index} 把，剩余 {} 把", list.len()))
            );
            Ok(())
        }
        Some(other) => {
            let key = if other == "add" {
                value.ok_or_else(|| anyhow!("用法: ccms key {provider_name} add <sk-...>"))?
            } else {
                other
            };
            let key = key.trim();
            if key.is_empty() {
                bail!("key 不能为空");
            }
            if existing.iter().any(|current| current == key) {
                println!("{}", style.warn("该 key 已在池中"));
                return Ok(());
            }
            let mut list = existing.clone();
            list.push(key.to_string());
            write_pool(&list)?;
            println!(
                "{}",
                style.ok(&format!(
                    "{provider_name} 密钥池 {} 把（新增 {}）",
                    list.len(),
                    super::mask(key)
                ))
            );
            Ok(())
        }
    }
}
