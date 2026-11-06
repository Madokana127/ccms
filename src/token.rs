use anyhow::{bail, Context, Result};
use axum::http::{header, HeaderMap};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use subtle::ConstantTimeEq;

use crate::paths::Paths;

const TOKEN_BYTES: usize = 32;
/// 32 字节 base64url（无填充）固定 43 字符。
const MIN_TOKEN_LEN: usize = 43;
/// 仅用于 token 初始化的跨进程锁，不在请求热路径使用。
const LOCK_NAME: &str = "router.token.lock";

fn generate() -> Result<String> {
    let mut buf = [0u8; TOKEN_BYTES];
    getrandom::getrandom(&mut buf)
        .map_err(|err| anyhow::anyhow!("生成 router token 失败: {err}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

struct TokenLock {
    file: File,
}

impl TokenLock {
    fn acquire(home: &Path) -> Result<Self> {
        fs::create_dir_all(home).with_context(|| format!("创建 {}", home.display()))?;
        let path = home.join(LOCK_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("打开 router token 锁失败: {}", path.display()))?;
        crate::fsutil::set_mode(&path, 0o600)?;
        <File as FileExt>::lock_exclusive(&file)
            .with_context(|| format!("获取 router token 锁失败: {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for TokenLock {
    fn drop(&mut self) {
        let _ = <File as FileExt>::unlock(&self.file);
    }
}

pub fn load(paths: &Paths) -> Result<String> {
    let text = fs::read_to_string(&paths.router_token)
        .with_context(|| format!("读取 router token 失败: {}", paths.router_token.display()))?;
    let token = text.trim();
    if token.len() < MIN_TOKEN_LEN
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!(
            "router token 文件格式非法: {}",
            paths.router_token.display()
        );
    }
    Ok(token.to_string())
}

/// 读取，不存在则原子创建。`ccms on` 与 `router run` 共用这一条路径。
/// 整个“检查存在→生成→写入→读取”过程持有跨进程排他锁，避免并发
/// ensure 读到尚未写完的临时文件。
pub fn ensure(paths: &Paths) -> Result<String> {
    paths
        .ensure()
        .with_context(|| format!("创建 {}", paths.home.display()))?;
    let _lock = TokenLock::acquire(&paths.home)?;

    if paths.router_token.is_file() {
        let token = load(paths)?;
        crate::fsutil::set_mode(&paths.router_token, 0o600).with_context(|| {
            format!(
                "收紧 router token 权限失败: {}",
                paths.router_token.display()
            )
        })?;
        return Ok(token);
    }

    let token = generate()?;
    crate::fsutil::write_atomic(&paths.router_token, format!("{token}\n").as_bytes(), 0o600)?;
    // 以磁盘上的文件为准，保证所有并发调用拿到同一个 token。
    load(paths)
}

/// `/v1/models` 与 proxy 共用：`x-api-key` 或 `Authorization: Bearer` 任一匹配即可。
pub fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let expected = token.as_bytes();
    if let Some(value) = headers.get("x-api-key") {
        if constant_time_eq(value.as_bytes(), expected) {
            return true;
        }
    }
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if let Ok(text) = value.to_str() {
            if let Some((scheme, candidate)) = text.split_once(' ') {
                if scheme.eq_ignore_ascii_case("bearer")
                    && constant_time_eq(candidate.trim().as_bytes(), expected)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use axum::http::HeaderValue;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn paths_in(home: std::path::PathBuf) -> Paths {
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

    fn temp_paths() -> Paths {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("ccms-token-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        paths_in(home)
    }

    fn assert_valid(token: &str) {
        assert!(token.len() >= MIN_TOKEN_LEN, "token 过短: {token:?}");
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "token 含非法字符: {token:?}"
        );
    }

    /// 子进程入口：由并发多进程测试显式以 `--ignored` 启动。
    #[test]
    #[ignore]
    fn ensure_child_helper() {
        let Ok(home) = std::env::var("CCMS_TEST_TOKEN_HOME") else {
            return;
        };
        let token = ensure(&paths_in(home.into())).unwrap();
        println!("TOKEN:{token}");
    }

    #[test]
    fn concurrent_threads_share_one_valid_token() {
        let paths = temp_paths();
        let mut handles = Vec::new();
        for _ in 0..8 {
            let paths = paths.clone();
            handles.push(std::thread::spawn(move || ensure(&paths).unwrap()));
        }
        let tokens: Vec<String> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        for token in &tokens {
            assert_eq!(token, &tokens[0], "并发 ensure 必须返回同一 token");
            assert_valid(token);
        }
        assert_eq!(load(&paths).unwrap(), tokens[0]);
    }

    #[test]
    fn concurrent_processes_share_one_valid_token() {
        let paths = temp_paths();
        let exe = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for _ in 0..6 {
            children.push(
                std::process::Command::new(&exe)
                    .args([
                        "--exact",
                        "token::tests::ensure_child_helper",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("CCMS_TEST_TOKEN_HOME", &paths.home)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        let mut tokens = Vec::new();
        for child in children {
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "子进程失败: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let token = stdout
                .lines()
                .find_map(|line| line.strip_prefix("TOKEN:"))
                .unwrap_or_else(|| panic!("子进程未输出 token: {stdout:?}"))
                .to_string();
            tokens.push(token);
        }
        for token in &tokens {
            assert_eq!(token, &tokens[0], "并发进程必须返回同一 token");
            assert_valid(token);
        }
        let disk = load(&paths).unwrap();
        assert_eq!(disk, tokens[0]);
        assert_valid(&disk);
    }

    #[test]
    fn failed_token_write_leaves_no_empty_target() {
        let paths = temp_paths();
        paths.ensure().unwrap();
        // 原子写的临时路径被目录占据，fs::write 必然失败。
        let tmp = paths
            .router_token
            .with_extension(format!("tmp.{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir(&tmp).unwrap();

        assert!(ensure(&paths).is_err(), "临时文件写入失败必须向上返回");
        assert!(!paths.router_token.exists(), "写失败不得留下空目标文件");
        fs::remove_dir(&tmp).unwrap();
        let token = ensure(&paths).unwrap();
        assert_valid(&token);
    }

    #[test]
    fn failed_token_write_keeps_another_process_complete_token() {
        let paths = temp_paths();
        paths.ensure().unwrap();
        let existing = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVW";
        fs::write(&paths.router_token, format!("{existing}\n")).unwrap();
        let tmp = paths
            .router_token
            .with_extension(format!("tmp.{}", std::process::id()));
        fs::create_dir(&tmp).unwrap();

        let token = ensure(&paths).unwrap();
        assert_eq!(token, existing, "已存在的完整 token 不得被替换或删除");
        assert_eq!(
            fs::read_to_string(&paths.router_token).unwrap().trim(),
            existing
        );
    }

    #[cfg(unix)]
    #[test]
    fn token_lock_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let paths = temp_paths();
        ensure(&paths).unwrap();
        let lock = paths.home.join("router.token.lock");
        assert!(lock.is_file(), "必须创建 router.token.lock");
        let mode = fs::metadata(&lock).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "router.token.lock 必须为 600");
    }

    #[test]
    fn token_is_random_url_safe_and_stable() {
        let paths = temp_paths();
        let first = ensure(&paths).unwrap();
        let second = ensure(&paths).unwrap();
        assert_eq!(first, second, "同一 CCMS_HOME 必须复用同一个 token");
        assert!(first.len() >= MIN_TOKEN_LEN);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));

        let other = ensure(&temp_paths()).unwrap();
        assert_ne!(first, other, "不同 CCMS_HOME 必须得到不同 token");
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let paths = temp_paths();
        ensure(&paths).unwrap();
        let mode = fs::metadata(&paths.router_token)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn existing_token_is_tightened_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let paths = temp_paths();
        fs::create_dir_all(&paths.home).unwrap();
        fs::write(
            &paths.router_token,
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVW\n",
        )
        .unwrap();
        fs::set_permissions(&paths.router_token, fs::Permissions::from_mode(0o644)).unwrap();

        ensure(&paths).unwrap();
        let mode = fs::metadata(&paths.router_token)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "已存在的 router.token 也必须收紧到 600");
    }

    #[test]
    fn auth_accepts_either_header_and_rejects_others() {
        let token = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ";
        let mut headers = HeaderMap::new();
        assert!(!authorized(&headers, token), "缺失凭据必须拒绝");

        headers.insert("x-api-key", HeaderValue::from_static(token));
        assert!(authorized(&headers, token));

        headers.clear();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert!(authorized(&headers, token));

        headers.clear();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("bearer {token}")).unwrap(),
        );
        assert!(authorized(&headers, token));

        headers.clear();
        headers.insert("x-api-key", HeaderValue::from_static("wrong-token"));
        assert!(!authorized(&headers, token), "错误 token 必须拒绝");

        headers.clear();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert!(!authorized(&headers, token), "非 Bearer 方案必须拒绝");
    }
}
