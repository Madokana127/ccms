use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// 原子写入：同目录临时文件 + rename，避免中断时留下半截 JSON。
/// 任一步失败都会清理临时文件，权限设置失败也会向上传播。
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("创建目录 {}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));

    let result = (|| -> Result<()> {
        fs::write(&tmp, bytes).with_context(|| format!("写入 {}", tmp.display()))?;
        set_mode(&tmp, mode)?;
        fs::rename(&tmp, path).with_context(|| format!("替换 {}", path.display()))?;
        set_mode(path, mode)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Unix 上收紧权限；Windows 依赖目录 ACL，不做额外处理。
pub fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("设置权限 {:o}: {}", mode, path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccms-fsutil-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_atomic_cleans_temp_when_rename_fails() {
        let dir = temp_dir("cleanup");
        let dest = dir.join("dest");
        // 目标是目录时 rename 必然失败，用来模拟写入中途出错。
        fs::create_dir(&dest).unwrap();
        let tmp = dest.with_extension(format!("tmp.{}", std::process::id()));

        assert!(write_atomic(&dest, b"x", 0o600).is_err());
        assert!(
            !tmp.exists(),
            "写入失败后必须删除临时文件: {}",
            tmp.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_sets_requested_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        let path = dir.join("secret");
        write_atomic(&path, b"data", 0o600).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn set_mode_propagates_errors() {
        let dir = temp_dir("setmode");
        let missing = dir.join("missing");
        assert!(set_mode(&missing, 0o600).is_err(), "chmod 失败不能静默忽略");
    }
}
