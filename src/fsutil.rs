use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// 原子写入：同目录临时文件 + rename，避免中断时留下半截 JSON。
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok();
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, bytes).with_context(|| format!("写入 {}", tmp.display()))?;
    set_mode(&tmp, mode);
    fs::rename(&tmp, path).with_context(|| format!("替换 {}", path.display()))?;
    set_mode(path, mode);
    Ok(())
}

/// Unix 上收紧权限；Windows 依赖目录 ACL，不做额外处理。
pub fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

pub fn mode_string(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
            .unwrap_or_else(|_| "???".into())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        "---".into()
    }
}
