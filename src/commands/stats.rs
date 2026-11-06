use std::io::IsTerminal;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::paths::Paths;
use crate::stats;
use crate::term::Style;

pub async fn run(paths: &Paths, arg: Option<&str>, json: bool, by: Option<&str>) -> Result<()> {
    let style = Style::detect();

    if matches!(arg, Some("reset")) {
        if !confirm("清空用量记录？[y/N] ")? {
            println!("已取消");
            return Ok(());
        }
        clear_stats(&paths.stats)?;
        println!("{}", style.ok("已清空用量记录"));
        return Ok(());
    }

    if let Some(dimension) = by {
        if !matches!(dimension, "model" | "provider" | "tier") {
            bail!("--by 只支持 model / provider / tier");
        }
    }

    let days: u32 = match arg {
        Some(text) => text
            .parse()
            .map_err(|_| anyhow::anyhow!("天数必须是数字: {text}"))?,
        None => 7,
    };

    let data = stats::load(&paths.stats)?;
    let report = stats::aggregate(&data, days);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", stats::render(&report, &style, by));
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    use std::io::Write;
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES"))
}

fn clear_stats(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("清空用量记录失败: {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_reports_errors_other_than_not_found() {
        let dir = std::env::temp_dir().join(format!(
            "ccms-stats-reset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        clear_stats(&dir.join("missing.json")).expect("NotFound 视为已清空");

        let as_dir = dir.join("stats.json");
        std::fs::create_dir(&as_dir).unwrap();
        let err = clear_stats(&as_dir).expect_err("目录不是统计文件，删除错误必须返回");
        assert!(
            format!("{err:#}").contains("清空用量记录"),
            "错误应指出清空失败：{err:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
