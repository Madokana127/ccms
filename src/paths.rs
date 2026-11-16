use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

/// 所有运行期路径的唯一来源。
///
/// 统一规则：``（兼容 ``），否则 `<home>/.config/alkanet`
/// （若不存在但存在 `<home>/.config/ccms`，仍写向 `alkanet`）。
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub config: PathBuf,
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
    #[allow(dead_code)]
    pub settings: PathBuf,
    #[allow(dead_code)]
    pub settings_previous: PathBuf,
}

impl Paths {
    pub fn load() -> Result<Self> {
        let home = match env_path("ALKANET_HOME").or_else(|| env_path("CCMS_HOME")) {
            Some(path) => path,
            None => home_dir()
                .ok_or_else(|| {
                    anyhow!(
                        "无法确定用户主目录：请设置 ALKANET_HOME 或 HOME/USERPROFILE；拒绝回退到当前目录"
                    )
                })?
                .join(".config")
                .join("alkanet"),
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
            config: home.join("config.toml"),
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

    #[test]
    fn ensure_tightens_home_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("alkanet-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();

        let paths = Paths {
            config: home.join("config.toml"),
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
        assert_eq!(mode, 0o700, "ALKANET_HOME 必须收紧到 700");
    }
}
