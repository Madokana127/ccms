use anyhow::Result;

use crate::cli::VaultCmd;
use crate::config;
use crate::keystore::{shell_quote, Vault};
use crate::paths::Paths;
use crate::proc;
use crate::term::Style;

pub async fn run(paths: &Paths, cmd: VaultCmd) -> Result<()> {
    let style = Style::detect();
    let vault = Vault::new(paths);

    match cmd {
        VaultCmd::Status => {
            let status = vault.status();
            println!("{}", style.section("密钥库"));
            if !status.encrypted {
                println!("  存储方式    明文 keys.env（未启用加密）");
                println!("  {}", style.warn("建议执行 ccms vault seal"));
                return Ok(());
            }
            println!(
                "  存储方式    {}  {}",
                paths.keys_enc.display(),
                status.cipher
            );
            println!(
                "  主密钥模式  {}",
                if status.mode == "passphrase" {
                    "passphrase（scrypt，不落盘）"
                } else {
                    "keyfile"
                }
            );
            if let Some(keyfile) = &status.keyfile {
                println!(
                    "  主密钥文件  {}  {}",
                    keyfile.display(),
                    status.keyfile_mode
                );
            }
            println!(
                "  密钥条目    {}",
                if status.decryptable {
                    status.entries.to_string()
                } else {
                    "解密失败".to_string()
                }
            );
            println!(
                "  可解密      {}",
                if status.decryptable {
                    "是"
                } else {
                    "否（主密钥/口令不匹配）"
                }
            );
            println!(
                "  更新于      {}",
                if status.updated.is_empty() {
                    "未知"
                } else {
                    &status.updated
                }
            );
            if let Some(err) = status.error {
                println!("  {}", style.fail(&err));
            }
            Ok(())
        }
        VaultCmd::Seal => {
            let (imported, entries) = vault.seal()?;
            println!(
                "{}",
                style.ok(&format!(
                    "已加密 {entries} 个条目（导入明文 {imported} 个）"
                ))
            );
            Ok(())
        }
        VaultCmd::Open => {
            eprintln!("! 以下为明文密钥，注意终端记录与重定向");
            for (name, value) in vault.load()? {
                println!("{name}={}", shell_quote(&value));
            }
            Ok(())
        }
        VaultCmd::Rekey => {
            let (mode, entries) = vault.rekey()?;
            println!(
                "{}",
                style.ok(&format!("主密钥已轮换：{mode}，{entries} 个条目"))
            );
            let routes = config::load_routes(&paths.routes)?;
            if proc::alive(&routes.listen).await {
                println!(
                    "{}",
                    style.dim("router 检测到 mtime 变化后自动重载密钥，无需重启")
                );
            }
            Ok(())
        }
    }
}
