use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

/// 所有运行期路径的唯一来源。
///
/// 统一规则（三平台一致）：`$CCMS_HOME`，否则 `<home>/.config/ccms`。
/// Claude Code 的设置固定为 `<home>/.claude/settings.json`，可用 `CLAUDE_SETTINGS` 覆盖。
/// 两者都没有显式覆盖且无法确定 home 时返回错误，绝不回退到当前目录。
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub routes: PathBuf,
    pub providers: PathBuf,
    pub keys_env: PathBuf,
    pub keys_enc: PathBuf,
    pub keyfile: PathBuf,
    pub lock: PathBuf,
    pub rekey_journal: PathBuf,
    pub rekey_enc_backup: PathBuf,
    pub rekey_key_backup: PathBuf,
    pub stats: PathBuf,
    pub log: PathBuf,
    pub pid: PathBuf,
    pub router_token: PathBuf,
    pub settings: PathBuf,
    pub settings_previous: PathBuf,
}

impl Paths {
    pub fn load() -> Result<Self> {
        let home = match env_path("CCMS_HOME") {
            Some(path) => path,
            None => home_dir()
                .ok_or_else(|| {
                    anyhow!(
                        "无法确定用户主目录：请设置 CCMS_HOME 或 HOME/USERPROFILE；拒绝回退到当前目录"
                    )
                })?
                .join(".config")
                .join("ccms"),
        };
        let settings = match env_path("CLAUDE_SETTINGS") {
            Some(path) => path,
            None => home_dir()
                .ok_or_else(|| {
                    anyhow!(
                        "无法确定 Claude 设置路径：请设置 CLAUDE_SETTINGS 或 HOME/USERPROFILE；拒绝回退到当前目录"
                    )
                })?
                .join(".claude")
                .join("settings.json"),
        };

        Ok(Self {
            routes: home.join("routes.json"),
            providers: home.join("providers.json"),
            keys_env: home.join("keys.env"),
            keys_enc: home.join("keys.enc"),
            keyfile: home.join("keystore.key"),
            lock: home.join("keystore.lock"),
            rekey_journal: home.join("keystore.rekey.journal"),
            rekey_enc_backup: home.join("keys.enc.rekey.bak"),
            rekey_key_backup: home.join("keystore.key.rekey.bak"),
            stats: home.join("stats.json"),
            log: home.join("router.log"),
            pid: home.join("router.pid"),
            router_token: home.join("router.token"),
            settings,
            settings_previous: home.join("settings.previous.json"),
            home,
        })
    }

    /// 配置目录只允许当前用户访问；Unix 上收紧到 700，Windows 依赖目录 ACL。
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.home)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.home, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

/// Windows 下 PowerShell 通常只有 USERPROFILE，git-bash / WSL 有 HOME。
pub fn home_dir() -> Option<PathBuf> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = env_path(key) {
            return Some(value);
        }
    }
    None
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn ensure_tightens_home_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("ccms-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();

        let paths = Paths {
            routes: home.join("routes.json"),
            providers: home.join("providers.json"),
            keys_env: home.join("keys.env"),
            keys_enc: home.join("keys.enc"),
            keyfile: home.join("keystore.key"),
            lock: home.join("keystore.lock"),
            rekey_journal: home.join("keystore.rekey.journal"),
            rekey_enc_backup: home.join("keys.enc.rekey.bak"),
            rekey_key_backup: home.join("keystore.key.rekey.bak"),
            stats: home.join("stats.json"),
            log: home.join("router.log"),
            pid: home.join("router.pid"),
            router_token: home.join("router.token"),
            settings: home.join("settings.json"),
            settings_previous: home.join("settings.previous.json"),
            home: home.clone(),
        };
        paths.ensure().unwrap();
        let mode = std::fs::metadata(&home).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "CCMS_HOME 必须收紧到 700");
    }
}
