use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::paths::Paths;

pub fn read_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn health_client() -> reqwest::Client {
    crate::http::local().unwrap_or_else(|_| reqwest::Client::new())
}

pub async fn health(listen: &str) -> Option<serde_json::Value> {
    let url = format!("http://{listen}/ccms/health");
    let response = health_client().get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<serde_json::Value>().await.ok()
}

pub async fn alive(listen: &str) -> bool {
    health(listen)
        .await
        .map(|value| value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
        .unwrap_or(false)
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
    let running = alive(listen).await;
    let Some(pid) = read_pid(&paths.pid) else {
        if running {
            anyhow::bail!(
                "router 在运行但缺少 {}，请手动结束进程后重试",
                paths.pid.display()
            );
        }
        return Ok(false);
    };

    if !running {
        let _ = fs::remove_file(&paths.pid);
        return Ok(false);
    }

    signal(pid, false);
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if !alive(listen).await {
            let _ = fs::remove_file(&paths.pid);
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    signal(pid, true);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = fs::remove_file(&paths.pid);
    Ok(!alive(listen).await)
}

#[cfg(unix)]
fn signal(pid: u32, force: bool) {
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
    unsafe {
        libc::kill(pid as i32, sig);
    }
}

#[cfg(windows)]
fn signal(pid: u32, force: bool) {
    let _ = force;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let pid = pid.to_string();
    let _ = Command::new("taskkill")
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

pub fn rotate_log(path: &Path, max_bytes: u64) {
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > max_bytes {
            let backup = path.with_extension("log.1");
            let _ = fs::rename(path, backup);
        }
    }
}

pub fn tail(path: &Path, lines: usize) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines.max(1));
    all[start..].join("\n")
}
