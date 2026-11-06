use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Provider;
use crate::fsutil;
use crate::paths::Paths;

const ALGO: &str = "aes-256-gcm";
const VERSION: u32 = 1;
const KEY_LEN: usize = 32;
const IV_LEN: usize = 12;
const TAG_LEN: usize = 16;
const SALT_LEN: usize = 16;

/// Node 的 scryptSync 默认 N=16384（log_n=14）、r=8、p=1；保持一致才能读旧库。
const SCRYPT_LOG_N: u8 = 14;

#[derive(Serialize, Deserialize)]
struct Meta {
    version: u32,
    cipher: String,
    mode: String,
    iv: String,
    tag: String,
    data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kdf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    salt: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    updated: String,
}

/// 加密密钥池。密文与主密钥分离：
/// - keyfile 模式：`keystore.key` 存 32 字节随机主密钥（600）
/// - passphrase 模式：`CCMS_KEYSTORE_PASSPHRASE` 经 scrypt 派生，主密钥不落盘
pub struct Vault {
    pub enc: PathBuf,
    pub keyfile: PathBuf,
    pub legacy: PathBuf,
}

impl Vault {
    pub fn new(paths: &Paths) -> Self {
        Self {
            enc: paths.keys_enc.clone(),
            keyfile: paths.keyfile.clone(),
            legacy: paths.keys_env.clone(),
        }
    }

    pub fn exists(&self) -> bool {
        self.enc.is_file()
    }

    fn passphrase() -> Option<String> {
        env::var("CCMS_KEYSTORE_PASSPHRASE")
            .ok()
            .filter(|v| !v.is_empty())
    }

    fn read_meta(&self) -> Result<Meta> {
        let text = fs::read_to_string(&self.enc)
            .with_context(|| format!("读取密钥库失败: {}", self.enc.display()))?;
        let meta: Meta = serde_json::from_str(&text)
            .with_context(|| format!("密钥库格式非法: {}", self.enc.display()))?;
        if meta.version != VERSION {
            bail!("不支持的密钥库版本 {}", meta.version);
        }
        if meta.cipher != ALGO {
            bail!("不支持的加密算法 {}", meta.cipher);
        }
        Ok(meta)
    }

    fn keyfile_bytes(&self) -> Result<Vec<u8>> {
        let text = fs::read_to_string(&self.keyfile).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow!(
                    "主密钥文件缺失: {}（keys.enc 与 keystore.key 必须一起备份/迁移）",
                    self.keyfile.display()
                )
            } else {
                anyhow!("读取主密钥失败: {err}")
            }
        })?;
        let bytes = B64.decode(text.trim()).context("主密钥不是合法的 base64")?;
        if bytes.len() != KEY_LEN {
            bail!(
                "主密钥文件损坏: {}（应为 {KEY_LEN} 字节 base64）",
                self.keyfile.display()
            );
        }
        Ok(bytes)
    }

    fn ensure_keyfile(&self) -> Result<Vec<u8>> {
        if self.keyfile.is_file() {
            return self.keyfile_bytes();
        }
        let key = random_bytes(KEY_LEN)?;
        fsutil::write_atomic(&self.keyfile, B64.encode(&key).as_bytes(), 0o600)?;
        Ok(key)
    }

    fn master_key(&self, mode: &str, salt: Option<&[u8]>) -> Result<Vec<u8>> {
        if mode != "passphrase" {
            return self.keyfile_bytes();
        }
        let pass = Self::passphrase()
            .ok_or_else(|| anyhow!("该密钥库使用口令模式，但未设置 CCMS_KEYSTORE_PASSPHRASE"))?;
        let salt = salt.ok_or_else(|| anyhow!("密钥库缺少 scrypt salt"))?;
        let params = scrypt::Params::new(SCRYPT_LOG_N, 8, 1, KEY_LEN)
            .map_err(|_| anyhow!("scrypt 参数非法"))?;
        let mut out = vec![0u8; KEY_LEN];
        scrypt::scrypt(pass.as_bytes(), salt, &params, &mut out)
            .map_err(|_| anyhow!("scrypt 派生失败"))?;
        Ok(out)
    }

    /// 解密；未启用加密时回退读明文 keys.env（仅用于迁移）。
    pub fn load(&self) -> Result<BTreeMap<String, String>> {
        if self.exists() {
            self.decrypt()
        } else {
            Ok(read_env_file(&self.legacy))
        }
    }

    pub fn load_or_empty(&self) -> BTreeMap<String, String> {
        self.load().unwrap_or_default()
    }

    fn decrypt(&self) -> Result<BTreeMap<String, String>> {
        let meta = self.read_meta()?;
        let salt = match &meta.salt {
            Some(text) => Some(B64.decode(text).context("密钥库 salt 非法")?),
            None => None,
        };
        let key = self.master_key(&meta.mode, salt.as_deref())?;
        let iv = B64.decode(&meta.iv).context("密钥库 iv 非法")?;
        let tag = B64.decode(&meta.tag).context("密钥库 tag 非法")?;
        let mut data = B64.decode(&meta.data).context("密钥库 data 非法")?;
        if iv.len() != IV_LEN || tag.len() != TAG_LEN {
            bail!("密钥库 iv/tag 长度非法");
        }
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("主密钥长度非法"))?;
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&iv),
                aad(&meta.mode).as_slice(),
                &mut data,
                Tag::from_slice(&tag),
            )
            .map_err(|_| anyhow!("解密失败：主密钥不匹配、口令错误或密钥库已损坏"))?;
        let map: BTreeMap<String, String> =
            serde_json::from_slice(&data).context("密钥库内容不是合法的键值对象")?;
        Ok(map)
    }

    fn write_vault(
        &self,
        map: &BTreeMap<String, String>,
        mode: &str,
        key: &[u8],
        salt: Option<&[u8]>,
    ) -> Result<()> {
        let iv = random_bytes(IV_LEN)?;
        let mut data = serde_json::to_vec(map)?;
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("主密钥长度非法"))?;
        let tag = cipher
            .encrypt_in_place_detached(Nonce::from_slice(&iv), aad(mode).as_slice(), &mut data)
            .map_err(|_| anyhow!("加密失败"))?;
        let meta = Meta {
            version: VERSION,
            cipher: ALGO.into(),
            mode: mode.into(),
            iv: B64.encode(iv),
            tag: B64.encode(tag),
            data: B64.encode(&data),
            kdf: (mode == "passphrase").then(|| "scrypt".to_string()),
            salt: salt.map(|value| B64.encode(value)),
            updated: Utc::now().to_rfc3339(),
        };
        fsutil::write_atomic(
            &self.enc,
            serde_json::to_vec_pretty(&meta)?.as_slice(),
            0o600,
        )
    }

    /// 写入路径：已有密钥库沿用原模式与主密钥；否则按环境决定模式。
    fn persist(&self, map: &BTreeMap<String, String>) -> Result<()> {
        if self.exists() {
            let meta = self.read_meta()?;
            let salt = match &meta.salt {
                Some(text) => Some(B64.decode(text).context("密钥库 salt 非法")?),
                None => None,
            };
            let key = self.master_key(&meta.mode, salt.as_deref())?;
            self.write_vault(map, &meta.mode, &key, salt.as_deref())?;
        } else if Self::passphrase().is_some() {
            let salt = random_bytes(SALT_LEN)?;
            let key = self.master_key("passphrase", Some(&salt))?;
            self.write_vault(map, "passphrase", &key, Some(&salt))?;
        } else {
            let key = self.ensure_keyfile()?;
            self.write_vault(map, "keyfile", &key, None)?;
        }
        neutralize_legacy(&self.legacy)?;
        Ok(())
    }

    pub fn update<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut BTreeMap<String, String>),
    {
        let mut map = self.load()?;
        f(&mut map);
        self.persist(&map)
    }

    /// 把明文 keys.env 导入加密库；已加密时保留加密库中较新的值。
    pub fn seal(&self) -> Result<(usize, usize)> {
        let legacy = read_env_file(&self.legacy);
        let current = if self.exists() {
            self.decrypt()?
        } else {
            BTreeMap::new()
        };
        let imported = legacy.len();
        let mut merged = legacy;
        merged.extend(current);
        self.persist(&merged)?;
        Ok((imported, merged.len()))
    }

    /// 首次运行时自动迁移明文密钥；已加密或没有明文时返回 None。
    pub fn migrate_legacy(&self) -> Result<Option<usize>> {
        if self.exists() {
            return Ok(None);
        }
        let legacy = read_env_file(&self.legacy);
        if legacy.is_empty() || legacy.values().all(|v| v.trim().is_empty()) {
            return Ok(None);
        }
        self.persist(&legacy)?;
        Ok(Some(legacy.len()))
    }

    /// 轮换主密钥 / 切换模式。任一步失败都回滚旧字节。
    pub fn rekey(&self) -> Result<(String, usize)> {
        let map = self.load()?;
        let mode = if Self::passphrase().is_some() {
            "passphrase"
        } else {
            "keyfile"
        };
        let old_enc = fs::read(&self.enc).ok();
        let old_key = fs::read(&self.keyfile).ok();

        let result = (|| -> Result<()> {
            if mode == "passphrase" {
                let salt = random_bytes(SALT_LEN)?;
                let key = self.master_key(mode, Some(&salt))?;
                self.write_vault(&map, mode, &key, Some(&salt))
            } else {
                let key = random_bytes(KEY_LEN)?;
                self.write_vault(&map, mode, &key, None)?;
                fsutil::write_atomic(&self.keyfile, B64.encode(&key).as_bytes(), 0o600)
            }
        })();

        if let Err(err) = result {
            if let Some(bytes) = old_enc {
                let _ = fs::write(&self.enc, bytes);
            }
            if let Some(bytes) = old_key {
                let _ = fs::write(&self.keyfile, bytes);
            }
            return Err(anyhow!("主密钥轮换失败，已回滚: {err}"));
        }

        neutralize_legacy(&self.legacy)?;
        Ok((mode.to_string(), map.len()))
    }

    pub fn status(&self) -> VaultStatus {
        if !self.exists() {
            return VaultStatus {
                encrypted: false,
                ..Default::default()
            };
        }
        let mut status = VaultStatus {
            encrypted: true,
            ..Default::default()
        };
        match self.read_meta() {
            Ok(meta) => {
                status.cipher = meta.cipher;
                status.mode = meta.mode.clone();
                status.updated = meta.updated;
                if meta.mode != "passphrase" {
                    status.keyfile = Some(self.keyfile.clone());
                    status.keyfile_mode = fsutil::mode_string(&self.keyfile);
                }
                match self.decrypt() {
                    Ok(map) => {
                        status.entries = map.len();
                        status.decryptable = true;
                    }
                    Err(err) => status.error = Some(format!("{err:#}")),
                }
            }
            Err(err) => status.error = Some(format!("{err:#}")),
        }
        status
    }
}

#[derive(Default, Debug)]
pub struct VaultStatus {
    pub encrypted: bool,
    pub cipher: String,
    pub mode: String,
    pub keyfile: Option<PathBuf>,
    pub keyfile_mode: String,
    pub entries: usize,
    pub decryptable: bool,
    pub updated: String,
    pub error: Option<String>,
}

fn random_bytes(len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    getrandom::getrandom(&mut buf).map_err(|err| anyhow!("生成随机数失败: {err}"))?;
    Ok(buf)
}

/// AAD 绑定版本与模式：篡改元数据会让 GCM 校验失败，而不是被静默接受。
fn aad(mode: &str) -> Vec<u8> {
    format!("ccms-keystore:{VERSION}:{mode}").into_bytes()
}

fn valid_var(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn read_env_file(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !valid_var(name) {
            continue;
        }
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        out.insert(name.to_string(), value);
    }
    out
}

/// 迁移后抹掉明文，但保留文件，避免依赖「文件存在」的旧脚本报错。
fn neutralize_legacy(path: &Path) -> Result<()> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(());
    };
    let has_secret = text.lines().any(|line| {
        let line = line.trim();
        !line.is_empty()
            && !line.starts_with('#')
            && line
                .split_once('=')
                .map(|(_, v)| !v.trim().is_empty())
                .unwrap_or(false)
    });
    if !has_secret {
        return Ok(());
    }
    let body = "# 密钥已迁移到 keys.enc（AES-256-GCM），此文件不再保存明文。\n\
                # 重新导入：手动填入 VAR=值 后执行 ccms vault seal\n";
    fsutil::write_atomic(path, body.as_bytes(), 0o600)
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 从解密后的环境变量里取出某 provider 的密钥池。
pub fn keys_for(env: &BTreeMap<String, String>, provider: &Provider) -> Vec<String> {
    let mut raw = provider
        .pool_var()
        .and_then(|var| env.get(&var).cloned())
        .unwrap_or_default();
    if raw.trim().is_empty() {
        if let Some(var) = &provider.key_env {
            raw = env.get(var).cloned().unwrap_or_default();
        }
    }
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|key| !key.is_empty() && !key.contains("REPLACE_ME") && *key != "sk-")
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    /// 口令模式测试会改进程环境变量，串行化避免互相干扰。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn put(vault: &Vault, name: &str, value: &str) {
        vault
            .update(|map| {
                map.insert(name.to_string(), value.to_string());
            })
            .unwrap();
    }

    fn fetch(vault: &Vault, name: &str) -> Option<String> {
        vault.load().ok()?.get(name).cloned()
    }

    fn temp_paths() -> Paths {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("ccms-keystore-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        Paths {
            routes: home.join("routes.json"),
            providers: home.join("providers.json"),
            keys_env: home.join("keys.env"),
            keys_enc: home.join("keys.enc"),
            keyfile: home.join("keystore.key"),
            stats: home.join("stats.json"),
            log: home.join("router.log"),
            pid: home.join("router.pid"),
            settings: home.join("settings.json"),
            home,
        }
    }

    #[test]
    fn roundtrip_and_tamper() {
        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        put(&vault, "KIMI_API_KEYS", "sk-a,sk-b");
        assert!(vault.exists());
        assert_eq!(fetch(&vault, "KIMI_API_KEYS").as_deref(), Some("sk-a,sk-b"));

        let text = fs::read_to_string(&paths.keys_enc).unwrap();
        let mut meta: serde_json::Value = serde_json::from_str(&text).unwrap();
        meta["mode"] = serde_json::json!("passphrase");
        fs::write(&paths.keys_enc, meta.to_string()).unwrap();
        assert!(vault.load().is_err(), "篡改 AAD 后必须解密失败");
    }

    #[test]
    fn wrong_passphrase_fails_closed() {
        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        env::set_var("CCMS_KEYSTORE_PASSPHRASE", "correct horse");
        put(&vault, "KIMI_API_KEYS", "sk-secret");
        env::remove_var("CCMS_KEYSTORE_PASSPHRASE");
        assert!(vault.load().is_err(), "缺口令不得解密");
        env::set_var("CCMS_KEYSTORE_PASSPHRASE", "wrong");
        assert!(vault.load().is_err(), "错误口令不得解密");
        env::set_var("CCMS_KEYSTORE_PASSPHRASE", "correct horse");
        assert_eq!(fetch(&vault, "KIMI_API_KEYS").as_deref(), Some("sk-secret"));
        assert!(!paths.keyfile.exists(), "口令模式不得落盘主密钥");
        env::remove_var("CCMS_KEYSTORE_PASSPHRASE");
    }

    #[test]
    fn legacy_plaintext_migrates_once() {
        let _guard = guard();
        let paths = temp_paths();
        fs::write(&paths.keys_env, "KIMI_API_KEYS=sk-old\n").unwrap();
        let vault = Vault::new(&paths);
        assert_eq!(vault.load().unwrap()["KIMI_API_KEYS"], "sk-old");

        put(&vault, "DEEPSEEK_API_KEYS", "sk-new");
        assert!(vault.exists());
        let left = fs::read_to_string(&paths.keys_env).unwrap();
        assert!(!left.contains("sk-old"), "明文必须清除");
        assert_eq!(fetch(&vault, "KIMI_API_KEYS").as_deref(), Some("sk-old"));
    }

    #[test]
    fn rekey_keeps_data() {
        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        put(&vault, "KIMI_API_KEYS", "sk-a,sk-b");
        vault.rekey().unwrap();
        assert_eq!(fetch(&vault, "KIMI_API_KEYS").as_deref(), Some("sk-a,sk-b"));
    }

    /// 与 0.1 版 Node 实现产出的密钥库双向兼容（fixture 由旧 keystore.js 生成）。
    #[test]
    fn reads_node_vault_fixture() {
        let _guard = guard();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/js-vault");
        let paths = Paths {
            routes: fixture.join("routes.json"),
            providers: fixture.join("providers.json"),
            keys_env: fixture.join("keys.env"),
            keys_enc: fixture.join("keys.enc"),
            keyfile: fixture.join("keystore.key"),
            stats: fixture.join("stats.json"),
            log: fixture.join("router.log"),
            pid: fixture.join("router.pid"),
            settings: fixture.join("settings.json"),
            home: fixture.clone(),
        };
        let vault = Vault::new(&paths);
        let map = vault.load().expect("应能解密旧版密钥库");
        assert_eq!(
            map.get("KIMI_API_KEYS").map(String::as_str),
            Some("sk-fixture-alpha,sk-fixture-beta")
        );
    }

    #[test]
    fn pool_supports_single_and_multi_keys() {
        let provider = Provider {
            base_url: "https://x".into(),
            key_env: Some("KIMI_API_KEY".into()),
            ..Default::default()
        };
        let mut env = BTreeMap::new();
        env.insert("KIMI_API_KEY".into(), "sk-single".to_string());
        assert_eq!(keys_for(&env, &provider), vec!["sk-single"]);

        env.insert("KIMI_API_KEYS".into(), "sk-a, sk-b ,,".to_string());
        assert_eq!(keys_for(&env, &provider), vec!["sk-a", "sk-b"]);
    }
}
