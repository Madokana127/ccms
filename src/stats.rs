use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::fsutil;

const RETENTION_DAYS: usize = 180;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Bucket {
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub retries: u64,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub duration_ms: u64,
}

impl Bucket {
    fn add(&mut self, other: &Bucket) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.retries += other.retries;
        self.errors += other.errors;
        self.duration_ms += other.duration_ms;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_create: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Day {
    #[serde(default)]
    pub total: Bucket,
    #[serde(default)]
    pub providers: BTreeMap<String, Bucket>,
    #[serde(default)]
    pub tiers: BTreeMap<String, Bucket>,
    /// 按 `provider/model` 聚合——档位随时可改，模型维度才是真实成本来源。
    #[serde(default)]
    pub models: BTreeMap<String, Bucket>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stats {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub daily: BTreeMap<String, Day>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            version: default_version(),
            daily: BTreeMap::new(),
        }
    }
}

fn default_version() -> u32 {
    2
}

pub fn load(path: &Path) -> Result<Stats> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} 已损坏或不是合法统计 JSON", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Stats::default()),
        Err(err) => Err(err).with_context(|| format!("读取统计文件 {}", path.display())),
    }
}

pub fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

#[allow(clippy::too_many_arguments)]
pub fn record(
    path: &Path,
    tier: &str,
    provider: &str,
    model: &str,
    usage: Option<Usage>,
    retries: u64,
    error: bool,
    duration_ms: u64,
) -> Result<()> {
    let mut stats = load(path)?;
    let day = stats.daily.entry(today()).or_default();

    let mut delta = Bucket {
        requests: 1,
        retries,
        errors: u64::from(error),
        duration_ms,
        ..Default::default()
    };
    if let Some(usage) = usage {
        delta.input_tokens = usage.input;
        delta.output_tokens = usage.output;
        delta.cache_read_input_tokens = usage.cache_read;
        delta.cache_creation_input_tokens = usage.cache_create;
    }

    day.total.add(&delta);
    day.providers
        .entry(provider.to_string())
        .or_default()
        .add(&delta);
    day.tiers.entry(tier.to_string()).or_default().add(&delta);
    day.models
        .entry(format!("{provider}/{model}"))
        .or_default()
        .add(&delta);

    prune(&mut stats);
    let body = serde_json::to_vec(&stats).context("序列化统计失败")?;
    fsutil::write_atomic(path, &body, 0o644)
}

fn prune(stats: &mut Stats) {
    while stats.daily.len() > RETENTION_DAYS {
        let Some(oldest) = stats.daily.keys().next().cloned() else {
            break;
        };
        stats.daily.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_defaults_only_when_file_is_missing() {
        let dir = std::env::temp_dir().join(format!("ccms-stats-load-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.json");
        assert_eq!(
            load(&missing).unwrap().daily.len(),
            0,
            "NotFound 才返回空统计"
        );

        let not_a_file = dir.join("directory");
        fs::create_dir(&not_a_file).unwrap();
        assert!(load(&not_a_file).is_err(), "其他 I/O 错误必须向上传播");
    }

    #[test]
    fn corrupt_stats_are_reported_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("ccms-stats-corrupt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stats.json");
        let corrupt = br#"{"version":2,"daily":{"2026-01-01":{"total":"broken"#.to_vec();
        fs::write(&path, &corrupt).unwrap();

        let err = record(
            &path,
            "sonnet",
            "p",
            "m",
            Some(Usage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_create: 0,
            }),
            0,
            false,
            5,
        )
        .expect_err("损坏的 stats 不得被当成空文件覆盖");
        assert!(
            format!("{err:#}").contains("stats") || format!("{err:#}").contains("统计"),
            "错误应能定位 stats: {err:#}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            corrupt,
            "失败后原文件字节必须不变"
        );
    }
}
