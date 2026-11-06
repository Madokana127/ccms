use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
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

/// rekey 事务状态；磁盘上同名文件固定，未 committed 时下次读取会恢复旧密钥对。
#[derive(Serialize, Deserialize)]
struct RekeyJournal {
    state: String,
    had_enc: bool,
    had_keyfile: bool,
}

struct KeystoreLock {
    file: File,
}

impl KeystoreLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("创建密钥库目录 {}", dir.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("打开密钥库锁失败: {}", path.display()))?;
        fsutil::set_mode(path, 0o600)?;
        <File as FileExt>::lock_exclusive(&file)
            .with_context(|| format!("获取密钥库写锁失败: {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for KeystoreLock {
    fn drop(&mut self) {
        let _ = <File as FileExt>::unlock(&self.file);
    }
}

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
    pub lock: PathBuf,
    pub journal: PathBuf,
    pub enc_backup: PathBuf,
    pub key_backup: PathBuf,
}

impl Vault {
    pub fn new(paths: &Paths) -> Self {
        Self {
            enc: paths.keys_enc.clone(),
            keyfile: paths.keyfile.clone(),
            legacy: paths.keys_env.clone(),
            lock: paths.lock.clone(),
            journal: paths.rekey_journal.clone(),
            enc_backup: paths.rekey_enc_backup.clone(),
            key_backup: paths.rekey_key_backup.clone(),
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
    ///
    /// 读取本身不加全局锁（keys.enc 通过原子 rename 替换）。若存在未完成的
    /// rekey journal，则先取得写锁并恢复旧密钥对；解密失败时也会复查一次，
    /// 避免与刚结束的事务交错。
    pub fn load(&self) -> Result<BTreeMap<String, String>> {
        let mut last = None;
        for _ in 0..2 {
            if self.recovery_pending() {
                let _lock = self.acquire_lock()?;
                self.recover_locked()?;
                return self.load_locked();
            }
            match self.load_locked() {
                Ok(map) => return Ok(map),
                Err(err) => last = Some(err),
            }
        }
        Err(last.expect("循环至少执行一次"))
    }

    fn load_locked(&self) -> Result<BTreeMap<String, String>> {
        if self.exists() {
            self.decrypt()
        } else {
            read_env_file(&self.legacy)
        }
    }

    fn acquire_lock(&self) -> Result<KeystoreLock> {
        KeystoreLock::acquire(&self.lock)
    }

    fn recovery_pending(&self) -> bool {
        self.journal.is_file()
    }

    fn read_journal(&self) -> Result<Option<RekeyJournal>> {
        let text = match fs::read_to_string(&self.journal) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("读取 rekey 事务日志失败: {}", self.journal.display())
                })
            }
        };
        let journal = serde_json::from_str(&text)
            .with_context(|| format!("rekey 事务日志格式非法: {}", self.journal.display()))?;
        Ok(Some(journal))
    }

    fn write_journal(&self, state: &str, had_enc: bool, had_keyfile: bool) -> Result<()> {
        let journal = RekeyJournal {
            state: state.to_string(),
            had_enc,
            had_keyfile,
        };
        fsutil::write_atomic(
            &self.journal,
            serde_json::to_vec(&journal)?.as_slice(),
            0o600,
        )
        .with_context(|| format!("写入 rekey 事务日志失败: {}", self.journal.display()))
    }

    /// 事务中止或崩溃后的统一恢复入口；调用方必须持有写锁。
    fn recover_locked(&self) -> Result<()> {
        let Some(journal) = self.read_journal()? else {
            return Ok(());
        };
        match journal.state.as_str() {
            "committed" => self.clear_artifacts(true),
            "prepared" | "enc-written" => {
                self.restore_old_pair(&journal).with_context(|| {
                    "检测到未完成的主密钥轮换，恢复旧密钥失败（事务文件已保留，下次读取会重试）"
                })?;
                self.clear_artifacts(false)
            }
            other => bail!(
                "rekey 事务日志状态未知: {other:?}（{}）",
                self.journal.display()
            ),
        }
    }

    fn restore_old_pair(&self, journal: &RekeyJournal) -> Result<()> {
        let mut errors = Vec::new();

        if journal.had_enc {
            match fs::read(&self.enc_backup) {
                Ok(bytes) => {
                    if let Err(err) = fsutil::write_atomic(&self.enc, &bytes, 0o600) {
                        errors.push(format!("恢复 {} 失败: {err:#}", self.enc.display()));
                    }
                }
                Err(err) => errors.push(format!(
                    "读取备份 {} 失败: {err}",
                    self.enc_backup.display()
                )),
            }
        } else if let Err(err) = fs::remove_file(&self.enc) {
            if err.kind() != ErrorKind::NotFound {
                errors.push(format!("删除 {} 失败: {err}", self.enc.display()));
            }
        }

        if journal.had_keyfile {
            match fs::read(&self.key_backup) {
                Ok(bytes) => {
                    if let Err(err) = fsutil::write_atomic(&self.keyfile, &bytes, 0o600) {
                        errors.push(format!("恢复 {} 失败: {err:#}", self.keyfile.display()));
                    }
                }
                Err(err) => errors.push(format!(
                    "读取备份 {} 失败: {err}",
                    self.key_backup.display()
                )),
            }
        } else if let Err(err) = fs::remove_file(&self.keyfile) {
            if err.kind() != ErrorKind::NotFound {
                errors.push(format!("删除 {} 失败: {err}", self.keyfile.display()));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(errors.join("; ")))
        }
    }

    /// 清理顺序决定崩溃语义：committed 先删备份；回滚/恢复先删 journal。
    /// 后者保证即使清理中断，也不会留下“缺备份的未完成事务”。
    /// 遇到第一个非 NotFound 的删除错误立即停止，保留尚未删除的文件供下次重试。
    fn clear_artifacts(&self, backups_first: bool) -> Result<()> {
        let paths: [&PathBuf; 3] = if backups_first {
            [&self.enc_backup, &self.key_backup, &self.journal]
        } else {
            [&self.journal, &self.enc_backup, &self.key_backup]
        };
        for path in paths {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(anyhow!(
                        "清理密钥事务文件失败: 删除 {} 失败: {err}",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn write_backup(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        fsutil::write_atomic(path, bytes, 0o600)?;
        let check = fs::read(path).with_context(|| format!("复核备份失败: {}", path.display()))?;
        if check != bytes {
            bail!("备份不完整: {}", path.display());
        }
        Ok(())
    }

    fn prepare_backups(&self) -> Result<(bool, bool)> {
        let old_enc = match fs::read(&self.enc) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("读取旧密钥库失败: {}", self.enc.display()))
            }
        };
        let old_key = match fs::read(&self.keyfile) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("读取旧主密钥失败: {}", self.keyfile.display()))
            }
        };

        if let Some(bytes) = &old_enc {
            self.write_backup(&self.enc_backup, bytes)?;
        }
        if let Some(bytes) = &old_key {
            self.write_backup(&self.key_backup, bytes)?;
        }
        Ok((old_enc.is_some(), old_key.is_some()))
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
    /// 调用方必须持有写锁。
    fn persist_locked(&self, map: &BTreeMap<String, String>) -> Result<()> {
        // 先确认明文可读：不能让写入成功后半途发现 keys.env 不是 UTF-8。
        self.ensure_legacy_readable()?;
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

    fn ensure_legacy_readable(&self) -> Result<()> {
        match fs::read_to_string(&self.legacy) {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| {
                format!(
                    "读取明文密钥文件失败: {}（必须是 UTF-8）",
                    self.legacy.display()
                )
            }),
        }
    }

    /// 在写锁内执行完整的 load-modify-persist；闭包看到的是最新磁盘状态。
    pub fn update_with<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut BTreeMap<String, String>) -> Result<T>,
    {
        let _lock = self.acquire_lock()?;
        self.recover_locked()?;
        let mut map = self.load_locked()?;
        let out = f(&mut map)?;
        self.persist_locked(&map)?;
        Ok(out)
    }

    /// 把明文 keys.env 导入加密库；已加密时保留加密库中较新的值。
    pub fn seal(&self) -> Result<(usize, usize)> {
        let _lock = self.acquire_lock()?;
        self.recover_locked()?;
        let legacy = read_env_file(&self.legacy)?;
        let current = if self.exists() {
            self.decrypt()?
        } else {
            BTreeMap::new()
        };
        let imported = legacy.len();
        let mut merged = legacy;
        merged.extend(current);
        self.persist_locked(&merged)?;
        Ok((imported, merged.len()))
    }

    /// 首次运行时自动迁移明文密钥；已加密或没有明文时返回 None。
    pub fn migrate_legacy(&self) -> Result<Option<usize>> {
        let _lock = self.acquire_lock()?;
        self.recover_locked()?;
        if self.exists() {
            return Ok(None);
        }
        let legacy = read_env_file(&self.legacy)?;
        if legacy.is_empty() || legacy.values().all(|v| v.trim().is_empty()) {
            return Ok(None);
        }
        self.persist_locked(&legacy)?;
        Ok(Some(legacy.len()))
    }

    /// 轮换主密钥 / 切换模式。
    ///
    /// 通过固定 journal + 旧密文/旧 keyfile 备份实现崩溃可恢复：只有 committed
    /// journal 落盘后才保留新密钥对；任何未 committed 的状态都会在下次
    /// load/status/persist/migrate 时恢复旧密钥对。
    pub fn rekey(&self) -> Result<(String, usize)> {
        let _lock = self.acquire_lock()?;
        self.recover_locked()?;
        let map = self.load_locked()?;
        let mode = if Self::passphrase().is_some() {
            "passphrase"
        } else {
            "keyfile"
        };

        let prepared = self.prepare_backups().context("备份旧密钥失败，未开始轮换");
        let (had_enc, had_keyfile) = match prepared {
            Ok(prepared) => prepared,
            Err(err) => {
                let cleanup = self.clear_artifacts(false);
                return match cleanup {
                    Ok(()) => Err(err),
                    Err(cleanup) => Err(anyhow!(
                        "备份旧密钥失败，且清理不完整备份也失败: {err:#}; {cleanup:#}"
                    )),
                };
            }
        };
        if let Err(err) = self.write_journal("prepared", had_enc, had_keyfile) {
            let cleanup = self.clear_artifacts(false);
            return match cleanup {
                Ok(()) => Err(err.context("写入 rekey 事务日志失败，旧密钥未被修改")),
                Err(cleanup) => Err(anyhow!(
                    "写入 rekey 事务日志失败，且清理备份失败: {err:#}; {cleanup:#}"
                )),
            };
        }

        let result = (|| -> Result<()> {
            if mode == "passphrase" {
                let salt = random_bytes(SALT_LEN)?;
                let key = self.master_key(mode, Some(&salt))?;
                self.write_vault(&map, mode, &key, Some(&salt))?;
                self.write_journal("enc-written", had_enc, had_keyfile)?;
            } else {
                let key = random_bytes(KEY_LEN)?;
                self.write_vault(&map, mode, &key, None)?;
                self.write_journal("enc-written", had_enc, had_keyfile)?;
                fsutil::write_atomic(&self.keyfile, B64.encode(&key).as_bytes(), 0o600)?;
            }
            self.write_journal("committed", had_enc, had_keyfile)
        })();

        if let Err(err) = result {
            return self.rollback_rekey(err, had_enc, had_keyfile);
        }

        if let Err(err) = self.clear_artifacts(true) {
            return Err(anyhow!(
                "主密钥已轮换（已提交），但清理事务文件失败；下次读取会继续清理: {err:#}"
            ));
        }
        if let Err(err) = neutralize_legacy(&self.legacy) {
            return Err(anyhow!(
                "主密钥已轮换（已提交），但清理明文 keys.env 失败: {err:#}"
            ));
        }
        Ok((mode.to_string(), map.len()))
    }

    fn rollback_rekey(
        &self,
        cause: anyhow::Error,
        had_enc: bool,
        had_keyfile: bool,
    ) -> Result<(String, usize)> {
        let journal = RekeyJournal {
            state: "enc-written".into(),
            had_enc,
            had_keyfile,
        };
        match self.restore_old_pair(&journal) {
            Ok(()) => match self.clear_artifacts(false) {
                Ok(()) => Err(anyhow!("主密钥轮换失败，已回滚: {cause:#}")),
                Err(cleanup) => Err(anyhow!(
                    "主密钥轮换失败，已恢复旧密钥，但清理事务文件失败: {cause:#}; {cleanup:#}"
                )),
            },
            Err(restore) => Err(anyhow!(
                "主密钥轮换失败，恢复旧密钥也失败；事务文件已保留，下次读取会重试: \
                 原错误 {cause:#}; 恢复错误 {restore:#}"
            )),
        }
    }

    pub fn status(&self) -> VaultStatus {
        // 只在存在未完成事务时加锁恢复；正常读取不加全局锁。
        let guard = if self.recovery_pending() {
            match self.acquire_lock() {
                Ok(guard) => Some(guard),
                Err(err) => {
                    return VaultStatus {
                        encrypted: self.exists(),
                        error: Some(format!("{err:#}")),
                        ..Default::default()
                    }
                }
            }
        } else {
            None
        };
        if guard.is_some() {
            if let Err(err) = self.recover_locked() {
                return VaultStatus {
                    encrypted: self.exists(),
                    error: Some(format!("{err:#}")),
                    ..Default::default()
                };
            }
        }
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

pub fn read_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("读取明文密钥文件失败: {}（必须是 UTF-8）", path.display())
            })
        }
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
    Ok(out)
}

/// 迁移后抹掉明文，但保留文件，避免依赖「文件存在」的旧脚本报错。
/// 除 NotFound 外，任何读取错误都必须向上传播，不能假装已经清理完成。
fn neutralize_legacy(path: &Path) -> Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "读取明文密钥文件失败，无法确认是否已清理: {}（必须是 UTF-8）",
                    path.display()
                )
            })
        }
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
                # 重新导入：手动填入 VAR=值 后执行 ccms --vault --seal\n";
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

    struct PassphraseEnv;

    impl PassphraseEnv {
        fn set(value: &str) -> Self {
            env::set_var("CCMS_KEYSTORE_PASSPHRASE", value);
            Self
        }
    }

    impl Drop for PassphraseEnv {
        fn drop(&mut self) {
            env::remove_var("CCMS_KEYSTORE_PASSPHRASE");
        }
    }

    fn put(vault: &Vault, name: &str, value: &str) {
        vault
            .update_with(|map| {
                map.insert(name.to_string(), value.to_string());
                Ok(())
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
            lock: fixture.join("keystore.lock"),
            rekey_journal: fixture.join("keystore.rekey.journal"),
            rekey_enc_backup: fixture.join("keys.enc.rekey.bak"),
            rekey_key_backup: fixture.join("keystore.key.rekey.bak"),
            stats: fixture.join("stats.json"),
            log: fixture.join("router.log"),
            pid: fixture.join("router.pid"),
            router_token: fixture.join("router.token"),
            settings: fixture.join("settings.json"),
            settings_previous: fixture.join("settings.previous.json"),
            home: fixture.clone(),
        };
        let vault = Vault::new(&paths);
        let map = vault.load().expect("应能解密旧版密钥库");
        assert_eq!(
            map.get("KIMI_API_KEYS").map(String::as_str),
            Some("sk-fixture-alpha,sk-fixture-beta")
        );
    }

    fn artifact(paths: &Paths, name: &str) -> PathBuf {
        paths.home.join(name)
    }

    /// 构造 rekey 崩溃后的磁盘状态（不真正 kill 进程）。
    ///
    /// `state` 为 prepared / enc-written / committed；`new_keyfile` 表示崩溃时
    /// keystore.key 是否已经换成新主密钥。旧密钥对永远先通过正常写入建立。
    fn crash_state(state: &str, new_keyfile: bool) -> (Paths, Vault) {
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        put(&vault, "KIMI_API_KEYS", "sk-old");
        let old_enc = fs::read(&paths.keys_enc).unwrap();
        let old_key = fs::read(&paths.keyfile).unwrap();

        let mut new_map = BTreeMap::new();
        new_map.insert("KIMI_API_KEYS".to_string(), "sk-new".to_string());
        let new_key = random_bytes(KEY_LEN).unwrap();
        vault
            .write_vault(&new_map, "keyfile", &new_key, None)
            .unwrap();
        if new_keyfile {
            fsutil::write_atomic(&paths.keyfile, B64.encode(&new_key).as_bytes(), 0o600).unwrap();
        }
        fsutil::write_atomic(&artifact(&paths, "keys.enc.rekey.bak"), &old_enc, 0o600).unwrap();
        fsutil::write_atomic(&artifact(&paths, "keystore.key.rekey.bak"), &old_key, 0o600).unwrap();
        let journal = format!(r#"{{"state":"{state}","had_enc":true,"had_keyfile":true}}"#);
        fsutil::write_atomic(
            &artifact(&paths, "keystore.rekey.journal"),
            journal.as_bytes(),
            0o600,
        )
        .unwrap();
        (paths, vault)
    }

    fn assert_artifacts_cleaned(paths: &Paths) {
        for name in [
            "keystore.rekey.journal",
            "keys.enc.rekey.bak",
            "keystore.key.rekey.bak",
        ] {
            assert!(!artifact(paths, name).exists(), "事务结束后不应残留 {name}");
        }
    }

    #[test]
    fn non_utf8_legacy_fails_instead_of_empty_vault() {
        let _guard = guard();
        let paths = temp_paths();
        let invalid = b"KIMI_API_KEYS=sk-plain\n\xff\xfe\x00bad\n";
        fs::write(&paths.keys_env, invalid).unwrap();
        let vault = Vault::new(&paths);

        assert!(
            vault
                .update_with(|map| {
                    map.insert("NEW_KEY".into(), "sk-new".into());
                    Ok(())
                })
                .is_err(),
            "非 UTF-8 明文不能被当成空库"
        );
        assert!(!paths.keys_enc.exists(), "失败时不得创建 keys.enc");
        assert_eq!(fs::read(&paths.keys_env).unwrap(), invalid);

        assert!(vault.seal().is_err());
        assert!(!paths.keys_enc.exists());
        assert_eq!(fs::read(&paths.keys_env).unwrap(), invalid);

        assert!(vault.migrate_legacy().is_err());
        assert!(!paths.keys_enc.exists());
        assert_eq!(fs::read(&paths.keys_env).unwrap(), invalid);
    }

    #[test]
    fn non_utf8_legacy_blocks_neutralize_after_vault_exists() {
        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        put(&vault, "KIMI_API_KEYS", "sk-old");
        let before = fs::read(&paths.keys_enc).unwrap();

        let invalid = b"\xff\xfe not utf8\n";
        fs::write(&paths.keys_env, invalid).unwrap();
        assert!(
            vault
                .update_with(|map| {
                    map.insert("KIMI_API_KEYS".into(), "sk-changed".into());
                    Ok(())
                })
                .is_err(),
            "明文清理遇到读取错误必须失败"
        );
        assert_eq!(
            fs::read(&paths.keys_enc).unwrap(),
            before,
            "清理失败不得留下已改写的密钥库"
        );
        assert_eq!(fs::read(&paths.keys_env).unwrap(), invalid);
    }

    #[test]
    fn prepared_rekey_recovers_old_pair() {
        let _guard = guard();
        let (paths, vault) = crash_state("prepared", false);
        let map = vault.load().expect("prepared 状态下次读取必须恢复旧密钥对");
        assert_eq!(map["KIMI_API_KEYS"], "sk-old");
        assert_artifacts_cleaned(&paths);
    }

    #[test]
    fn enc_written_rekey_recovers_with_old_keyfile() {
        let _guard = guard();
        let (paths, vault) = crash_state("enc-written", false);
        let map = vault
            .load()
            .expect("enc-written 且 keyfile 未换时必须恢复旧值");
        assert_eq!(map["KIMI_API_KEYS"], "sk-old");
        assert_artifacts_cleaned(&paths);
    }

    #[test]
    fn enc_written_rekey_recovers_with_new_keyfile() {
        let _guard = guard();
        let (paths, vault) = crash_state("enc-written", true);
        let map = vault
            .load()
            .expect("enc-written 且 keyfile 已换时仍必须恢复旧值");
        assert_eq!(map["KIMI_API_KEYS"], "sk-old");
        assert_artifacts_cleaned(&paths);
    }

    #[test]
    fn status_and_update_trigger_recovery() {
        let _guard = guard();

        let (status_paths, status_vault) = crash_state("enc-written", true);
        let status = status_vault.status();
        assert!(status.encrypted && status.decryptable);
        assert_eq!(status.entries, 1);
        assert_eq!(
            fetch(&status_vault, "KIMI_API_KEYS").as_deref(),
            Some("sk-old")
        );
        assert_artifacts_cleaned(&status_paths);

        let (update_paths, update_vault) = crash_state("prepared", false);
        update_vault
            .update_with(|map| {
                map.insert("EXTRA_KEY".into(), "sk-extra".into());
                Ok(())
            })
            .unwrap();
        let map = update_vault.load().unwrap();
        assert_eq!(map["KIMI_API_KEYS"], "sk-old");
        assert_eq!(map["EXTRA_KEY"], "sk-extra");
        assert_artifacts_cleaned(&update_paths);

        let (migrate_paths, migrate_vault) = crash_state("prepared", true);
        assert_eq!(migrate_vault.migrate_legacy().unwrap(), None);
        assert_eq!(
            fetch(&migrate_vault, "KIMI_API_KEYS").as_deref(),
            Some("sk-old")
        );
        assert_artifacts_cleaned(&migrate_paths);
    }

    #[test]
    fn committed_rekey_keeps_new_pair() {
        let _guard = guard();
        let (paths, vault) = crash_state("committed", true);
        let map = vault.load().expect("committed 状态必须保留新密钥对");
        assert_eq!(map["KIMI_API_KEYS"], "sk-new");
        assert_artifacts_cleaned(&paths);
    }

    #[test]
    fn prepared_passphrase_rekey_recovers_old_ciphertext() {
        let _guard = guard();
        let _pass = PassphraseEnv::set("passphrase for rekey recovery");

        let paths = temp_paths();
        let vault = Vault::new(&paths);
        put(&vault, "KIMI_API_KEYS", "sk-old");
        let old_enc = fs::read(&paths.keys_enc).unwrap();

        let salt = random_bytes(SALT_LEN).unwrap();
        let key = vault.master_key("passphrase", Some(&salt)).unwrap();
        let mut new_map = BTreeMap::new();
        new_map.insert("KIMI_API_KEYS".to_string(), "sk-new".to_string());
        vault
            .write_vault(&new_map, "passphrase", &key, Some(&salt))
            .unwrap();
        fsutil::write_atomic(&artifact(&paths, "keys.enc.rekey.bak"), &old_enc, 0o600).unwrap();
        fsutil::write_atomic(
            &artifact(&paths, "keystore.rekey.journal"),
            br#"{"state":"prepared","had_enc":true,"had_keyfile":false}"#,
            0o600,
        )
        .unwrap();

        let map = vault.load().expect("口令模式 prepared 必须恢复旧密文");
        assert_eq!(map["KIMI_API_KEYS"], "sk-old");
        assert!(!paths.keyfile.exists(), "口令模式恢复不得生成 keyfile");
        assert_artifacts_cleaned(&paths);
    }

    #[test]
    fn normal_rekey_leaves_no_transaction_artifacts() {
        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        put(&vault, "KIMI_API_KEYS", "sk-a");
        vault.rekey().unwrap();
        assert_eq!(fetch(&vault, "KIMI_API_KEYS").as_deref(), Some("sk-a"));
        assert_artifacts_cleaned(&paths);
        assert!(paths.home.join("keystore.lock").exists(), "事务锁必须落盘");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(paths.home.join("keystore.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "锁文件必须为 600");
        }
    }

    #[cfg(unix)]
    #[test]
    fn transaction_secret_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        put(&vault, "KIMI_API_KEYS", "sk-a");

        let (had_enc, had_keyfile) = vault.prepare_backups().unwrap();
        assert!(had_enc && had_keyfile);
        vault
            .write_journal("prepared", had_enc, had_keyfile)
            .unwrap();

        for path in [&vault.enc_backup, &vault.key_backup, &vault.journal] {
            let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} 必须为 600", path.display());
        }

        vault.clear_artifacts(false).unwrap();
        assert_artifacts_cleaned(&paths);
    }

    #[test]
    fn rollback_cleanup_stops_at_first_delete_error_and_keeps_backups() {
        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        fs::create_dir_all(&paths.home).unwrap();
        // 目录占据 journal 路径，使回滚清理的第一个删除失败。
        fs::create_dir(&vault.journal).unwrap();
        fs::write(&vault.enc_backup, b"old enc").unwrap();
        fs::write(&vault.key_backup, b"old key").unwrap();

        assert!(
            vault.clear_artifacts(false).is_err(),
            "journal 删除失败必须返回错误"
        );
        assert!(
            vault.enc_backup.is_file(),
            "首个删除失败后不得继续删除密文备份"
        );
        assert!(
            vault.key_backup.is_file(),
            "首个删除失败后不得继续删除主密钥备份"
        );
    }

    #[test]
    fn committed_cleanup_stops_when_first_backup_delete_fails_and_keeps_journal() {
        let _guard = guard();
        let paths = temp_paths();
        let vault = Vault::new(&paths);
        fs::create_dir_all(&paths.home).unwrap();
        // 目录占据首个备份路径，使 committed 清理在第一步失败。
        fs::create_dir(&vault.enc_backup).unwrap();
        fs::write(&vault.key_backup, b"old key").unwrap();
        fs::write(&vault.journal, br#"{"state":"committed"}"#).unwrap();

        assert!(
            vault.clear_artifacts(true).is_err(),
            "备份删除失败必须返回错误"
        );
        assert!(
            vault.journal.is_file(),
            "首个备份删除失败后不得删除 journal，否则无法继续清理"
        );
        assert!(
            vault.key_backup.is_file(),
            "首个删除失败后不得继续删除后续备份"
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
