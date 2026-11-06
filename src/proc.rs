use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::paths::Paths;

pub fn read_pid(path: &Path) -> Result<Option<u32>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("读取 pidfile 失败: {}", path.display()))
        }
    };
    let text = text.trim();
    let pid: u32 = text
        .parse()
        .map_err(|_| anyhow::anyhow!("pidfile 内容非法: {text:?}（{}）", path.display()))?;
    if pid == 0 {
        bail!("pidfile 中 PID 必须为正数: {}", path.display());
    }
    if pid > i32::MAX as u32 {
        bail!(
            "pidfile 中 PID {pid} 超出 i32 范围，拒绝发送信号: {}",
            path.display()
        );
    }
    Ok(Some(pid))
}

pub fn health_client() -> Result<reqwest::Client> {
    crate::http::local()
}

pub async fn health(listen: &str) -> Result<Option<serde_json::Value>> {
    let url = format!("http://{listen}/ccms/health");
    let response = match health_client()?.get(url).send().await {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if !response.status().is_success() {
        return Ok(None);
    }
    Ok(response.json::<serde_json::Value>().await.ok())
}

fn is_healthy(value: &serde_json::Value) -> bool {
    value.get("ok").and_then(|v| v.as_bool()) == Some(true)
        && value.get("service").and_then(|v| v.as_str()) == Some("ccms")
}

fn verified_pid(health: &serde_json::Value, file_pid: Option<u32>) -> Result<u32> {
    let Some(file_pid) = file_pid else {
        anyhow::bail!("router 健康响应存在但缺少 pidfile，拒绝发送停止信号");
    };
    let health_pid = health
        .get("pid")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| anyhow::anyhow!("router 健康响应缺少 pid，拒绝发送停止信号"))?;
    if health_pid != file_pid {
        anyhow::bail!("health pid={health_pid} 与 pidfile pid={file_pid} 不一致，拒绝发送停止信号");
    }
    Ok(file_pid)
}

pub async fn alive(listen: &str) -> bool {
    matches!(
        health(listen).await,
        Ok(Some(value)) if is_healthy(&value)
    )
}

pub async fn wait_alive(listen: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if alive(listen).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    false
}

/// 以守护进程方式拉起 `ccms router run`，stdout/stderr 进 router.log。
pub fn spawn(paths: &Paths) -> Result<()> {
    rotate_log(&paths.log, 5 * 1024 * 1024);
    if let Some(dir) = paths.log.parent() {
        fs::create_dir_all(dir).ok();
    }
    let exe = std::env::current_exe().context("无法定位 ccms 可执行文件")?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("打开 {}", paths.log.display()))?;

    let mut cmd = Command::new(exe);
    cmd.arg("router")
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    cmd.spawn().context("启动 router 失败")?;
    Ok(())
}

/// 返回是否真的停掉了一个正在运行的 router。
pub async fn stop(paths: &Paths, listen: &str) -> Result<bool> {
    let health = health(listen).await?;
    let running = health.as_ref().is_some_and(is_healthy);
    let file_pid = read_pid(&paths.pid)?;

    if !running {
        if file_pid.is_some() {
            let _ = fs::remove_file(&paths.pid);
        }
        return Ok(false);
    }

    let pid = verified_pid(health.as_ref().expect("running 表示健康响应存在"), file_pid)?;

    signal(pid, false)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if !alive(listen).await {
            let _ = fs::remove_file(&paths.pid);
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    signal(pid, true)?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = fs::remove_file(&paths.pid);
    Ok(!alive(listen).await)
}

#[cfg(unix)]
fn signal(pid: u32, force: bool) -> Result<()> {
    let native_pid =
        i32::try_from(pid).with_context(|| format!("PID {pid} 超出 i32 范围，拒绝发送信号"))?;
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
    let rc = unsafe { libc::kill(native_pid, sig) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(err).with_context(|| {
            format!(
                "发送信号 {} 到 PID {native_pid} 失败",
                if force { "SIGKILL" } else { "SIGTERM" }
            )
        });
    }
    Ok(())
}

#[cfg(windows)]
fn signal(pid: u32, force: bool) -> Result<()> {
    let _ = force;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let pid = pid.to_string();
    let output = Command::new("taskkill")
        .args(["/PID", pid.as_str(), "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("启动 taskkill 失败: PID {pid}"))?;
    if !output.status.success() {
        bail!(
            "taskkill PID {pid} 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn rotate_log(path: &Path, max_bytes: u64) {
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > max_bytes {
            let backup = path.with_extension("log.1");
            let _ = fs::rename(path, backup);
        }
    }
}

pub fn tail(path: &Path, lines: usize) -> Result<String> {
    let wanted = lines.max(1);
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(String::new()),
        Err(err) => return Err(err).with_context(|| format!("打开日志失败: {}", path.display())),
    };
    let len = file
        .metadata()
        .with_context(|| format!("读取日志元数据失败: {}", path.display()))?
        .len();

    const CHUNK: u64 = 8 * 1024;
    const MAX_BUF: usize = 1024 * 1024;
    let mut buf: Vec<u8> = Vec::new();
    let mut pos = len;
    let mut newlines = 0usize;

    // 从文件尾按固定块反向读取，缓冲区最多 1 MiB。
    while pos > 0 && buf.len() < MAX_BUF && newlines < wanted {
        let read_len = CHUNK.min(pos).min((MAX_BUF - buf.len()) as u64);
        if read_len == 0 {
            break;
        }
        pos -= read_len;
        file.seek(SeekFrom::Start(pos))
            .with_context(|| format!("定位日志失败: {}", path.display()))?;
        let mut chunk = vec![0u8; read_len as usize];
        file.read_exact(&mut chunk)
            .with_context(|| format!("读取日志失败: {}", path.display()))?;
        newlines += chunk.iter().filter(|&&byte| byte == b'\n').count();
        chunk.extend_from_slice(&buf);
        buf = chunk;
    }

    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(wanted);
    Ok(all[start..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn fake_health(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("fake listen");
        let addr = listener.local_addr().expect("fake addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        addr.to_string()
    }

    #[tokio::test]
    async fn alive_requires_ccms_service_and_ok() {
        let foreign = fake_health(r#"{"ok":true,"service":"other","pid":1}"#).await;
        assert!(!alive(&foreign).await, "非 ccms 健康响应不得视为存活");

        let unhealthy = fake_health(r#"{"ok":false,"service":"ccms","pid":1}"#).await;
        assert!(!alive(&unhealthy).await, "ok=false 不得视为存活");
    }

    #[test]
    fn stop_pid_must_match_health_and_pidfile() {
        let health = serde_json::json!({"ok": true, "service": "ccms", "pid": 42});
        assert_eq!(verified_pid(&health, Some(42)).unwrap(), 42);

        let missing_file = verified_pid(&health, None).expect_err("缺少 pidfile 必须拒绝");
        assert!(format!("{missing_file:#}").contains("pidfile"));

        let mismatch = verified_pid(&health, Some(43)).expect_err("pid 不一致必须拒绝");
        assert!(format!("{mismatch:#}").contains("不一致"));

        let no_health_pid = serde_json::json!({"ok": true, "service": "ccms"});
        let missing_health =
            verified_pid(&no_health_pid, Some(42)).expect_err("health 缺 pid 必须拒绝");
        assert!(format!("{missing_health:#}").contains("缺少 pid"));
    }

    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let unique = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ccms-proc-{}-{unique}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_pid_accepts_positive_and_missing() {
        let dir = temp_dir("pid-valid");
        let path = dir.join("router.pid");
        fs::write(&path, " 4321\n").unwrap();
        assert_eq!(read_pid(&path).unwrap(), Some(4321));
        assert_eq!(read_pid(&dir.join("missing.pid")).unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_pid_rejects_values_that_must_never_be_signaled() {
        let dir = temp_dir("pid-invalid");
        let values = [
            "0",
            "00",
            "-1",
            "-42",
            "4294967296",
            "2147483648",
            "999999999999999999999",
            "abc",
            "",
            "   ",
        ];
        for (index, value) in values.iter().enumerate() {
            let path = dir.join(format!("pid-{index}"));
            fs::write(&path, value).unwrap();
            let err = read_pid(&path)
                .expect_err(&format!("pidfile 内容 {value:?} 必须被拒绝，绝不能发信号"));
            assert!(
                format!("{err:#}").contains("pidfile"),
                "{value:?} 的错误应指出 pidfile：{err:#}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn nonexistent_pid_signal_reports_error() {
        // Linux 的 pid_max 上限是 4194304，这个值不可能对应真实进程；
        // kill 只会返回 ESRCH，不会影响任何无关进程。
        let err = signal(2_000_000_000, false).expect_err("不存在的 PID 必须返回错误");
        assert!(
            format!("{err:#}").contains("信号"),
            "错误应指出发送信号失败：{err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn oversized_pid_is_rejected_before_kill() {
        let err = signal(i32::MAX as u32 + 1, true)
            .expect_err("超过 i32::MAX 的 PID 必须在 kill 前被拒绝");
        assert!(
            format!("{err:#}").contains("PID"),
            "错误应指出 PID 越界：{err:#}"
        );
    }

    #[test]
    fn tail_missing_file_is_empty() {
        let dir = temp_dir("tail-missing");
        assert_eq!(tail(&dir.join("none.log"), 10).unwrap(), "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_reads_bounded_end_of_large_file() {
        use std::io::Write as _;

        let dir = temp_dir("tail-large");
        let path = dir.join("router.log");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(b"first-marker-should-not-appear\n").unwrap();
        let filler = "z".repeat(200);
        for index in 0..9000 {
            writeln!(file, "{filler}-{index}").unwrap();
        }
        writeln!(file, "last-marker-ok").unwrap();
        drop(file);
        assert!(fs::metadata(&path).unwrap().len() > 1024 * 1024);

        let text = tail(&path, 3).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(
            text.contains("last-marker-ok"),
            "必须包含最后一行：{text:?}"
        );
        assert!(
            !text.contains("first-marker"),
            "只应读取末尾，不应包含文件开头：{text:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_is_lossy_and_propagates_non_notfound_errors() {
        let dir = temp_dir("tail-errors");
        let path = dir.join("router.log");
        fs::write(&path, b"\xff\xfe\nlast-ok\n").unwrap();
        assert_eq!(tail(&path, 1).unwrap(), "last-ok");

        let err = tail(&dir, 1).expect_err("目录不是普通文件，I/O 错误必须传播");
        assert!(!format!("{err:#}").is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
