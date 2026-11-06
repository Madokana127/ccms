use std::io::IsTerminal;

use anyhow::{bail, Result};

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
        let _ = std::fs::remove_file(&paths.stats);
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

    let data = stats::load(&paths.stats);
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
