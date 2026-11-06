use std::env;
use std::path::PathBuf;

/// 所有运行期路径的唯一来源。
///
/// 统一规则（三平台一致）：`$CCMS_HOME`，否则 `<home>/.config/ccms`。
/// Claude Code 的设置固定为 `<home>/.claude/settings.json`，可用 `CLAUDE_SETTINGS` 覆盖。
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub routes: PathBuf,
    pub providers: PathBuf,
    pub keys_env: PathBuf,
    pub keys_enc: PathBuf,
    pub keyfile: PathBuf,
    pub stats: PathBuf,
    pub log: PathBuf,
    pub pid: PathBuf,
    pub settings: PathBuf,
}

impl Paths {
    pub fn load() -> Self {
        let home = env_path("CCMS_HOME").unwrap_or_else(|| home_dir().join(".config").join("ccms"));
        let settings = env_path("CLAUDE_SETTINGS")
            .unwrap_or_else(|| home_dir().join(".claude").join("settings.json"));

        Self {
            routes: home.join("routes.json"),
            providers: home.join("providers.json"),
            keys_env: home.join("keys.env"),
            keys_enc: home.join("keys.enc"),
            keyfile: home.join("keystore.key"),
            stats: home.join("stats.json"),
            log: home.join("router.log"),
            pid: home.join("router.pid"),
            home,
            settings,
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.home)
    }
}

/// Windows 下 PowerShell 通常只有 USERPROFILE，git-bash / WSL 有 HOME。
pub fn home_dir() -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = env_path(key) {
            return value;
        }
    }
    PathBuf::from(".")
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
