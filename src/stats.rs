use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::fsutil;
use crate::term::Style;

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

    pub fn avg_ms(&self) -> u64 {
        self.duration_ms.checked_div(self.requests).unwrap_or(0)
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

pub fn load(path: &Path) -> Stats {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
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
    let mut stats = load(path);
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

#[derive(Serialize, Default)]
pub struct Report {
    pub today: String,
    pub days: u32,
    pub from: String,
    pub to: String,
    pub total: Bucket,
    pub today_total: Bucket,
    pub by_model: Vec<(String, Bucket)>,
    pub by_provider: Vec<(String, Bucket)>,
    pub by_tier: Vec<(String, Bucket)>,
}

impl Report {
    pub fn has_data(&self) -> bool {
        self.total.requests > 0
    }
}

pub fn aggregate(stats: &Stats, days: u32) -> Report {
    let days = days.max(1);
    let now = Local::now();
    let from = (now - chrono::Duration::days(i64::from(days) - 1))
        .format("%Y-%m-%d")
        .to_string();
    let today = now.format("%Y-%m-%d").to_string();

    let mut report = Report {
        today: today.clone(),
        days,
        from: from.clone(),
        to: today.clone(),
        ..Default::default()
    };

    let mut models: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut providers: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut tiers: BTreeMap<String, Bucket> = BTreeMap::new();

    for (date, day) in &stats.daily {
        if date.as_str() < from.as_str() || date.as_str() > today.as_str() {
            continue;
        }
        report.total.add(&day.total);
        merge_into(&mut models, &day.models);
        merge_into(&mut providers, &day.providers);
        merge_into(&mut tiers, &day.tiers);
    }

    if let Some(day) = stats.daily.get(&today) {
        report.today_total = day.total.clone();
    }

    report.by_model = sorted(models);
    report.by_provider = sorted(providers);
    report.by_tier = sorted(tiers);
    report
}

fn merge_into(target: &mut BTreeMap<String, Bucket>, source: &BTreeMap<String, Bucket>) {
    for (key, bucket) in source {
        target.entry(key.clone()).or_default().add(bucket);
    }
}

fn sorted(map: BTreeMap<String, Bucket>) -> Vec<(String, Bucket)> {
    let mut list: Vec<(String, Bucket)> = map.into_iter().collect();
    list.sort_by(|a, b| b.1.requests.cmp(&a.1.requests).then(a.0.cmp(&b.0)));
    list
}

pub fn render(report: &Report, style: &Style, only: Option<&str>) -> String {
    let mut out = String::new();
    if !report.has_data() {
        out.push_str("暂无用量记录（启用路由并产生请求后自动统计）\n");
        return out;
    }

    let show = |dim: &str| only.map(|o| o == dim).unwrap_or(true);

    if show("total") || only.is_none() {
        out.push_str(&block(
            &format!("Today  {}", report.today),
            &report.today_total,
            style,
        ));
        out.push('\n');
        out.push_str(&block(
            &format!(
                "Last {} days  ({} ~ {})",
                report.days, report.from, report.to
            ),
            &report.total,
            style,
        ));
    }

    if show("model") || only.is_none() {
        out.push_str(&table("By model", "MODEL", &report.by_model, style));
    }
    if show("provider") || only.is_none() {
        out.push_str(&table(
            "By provider",
            "PROVIDER",
            &report.by_provider,
            style,
        ));
    }
    if show("tier") || only.is_none() {
        out.push_str(&table("By tier", "TIER", &report.by_tier, style));
    }
    out
}

fn block(title: &str, bucket: &Bucket, style: &Style) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", style.bold(title)));
    out.push_str(&format!("  {:<10}{:>14}\n", "requests", n(bucket.requests)));
    out.push_str(&format!(
        "  {:<10}{:>14}\n",
        "input",
        n(bucket.input_tokens)
    ));
    out.push_str(&format!(
        "  {:<10}{:>14}\n",
        "output",
        n(bucket.output_tokens)
    ));
    if bucket.cache_read_input_tokens > 0 || bucket.cache_creation_input_tokens > 0 {
        out.push_str(&format!(
            "  {:<10}{:>14}\n",
            "cache r/w",
            format!(
                "{} / {}",
                n(bucket.cache_read_input_tokens),
                n(bucket.cache_creation_input_tokens)
            )
        ));
    }
    if bucket.retries > 0 || bucket.errors > 0 {
        out.push_str(&format!(
            "  {:<10}{:>14}\n",
            "retries",
            format!("{} (err {})", n(bucket.retries), n(bucket.errors))
        ));
    }
    out.push_str(&format!("  {:<10}{:>14}\n", "avg ms", n(bucket.avg_ms())));
    out
}

fn table(title: &str, head: &str, rows: &[(String, Bucket)], style: &Style) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!("\n{}\n", style.bold(title)));
    out.push_str(&format!(
        "  {:<30}{:>7}{:>14}{:>12}{:>9}\n",
        head, "REQ", "INPUT", "OUTPUT", "AVG MS"
    ));
    for (name, bucket) in rows {
        out.push_str(&format!(
            "  {:<30}{:>7}{:>14}{:>12}{:>9}\n",
            name,
            n(bucket.requests),
            n(bucket.input_tokens),
            n(bucket.output_tokens),
            n(bucket.avg_ms())
        ));
    }
    out
}

fn n(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_by_model() {
        let mut stats = Stats::default();
        let mut day = Day::default();
        day.total.requests = 2;
        day.models.insert(
            "kimi/kimi-k3".into(),
            Bucket {
                requests: 1,
                input_tokens: 100,
                output_tokens: 20,
                duration_ms: 300,
                ..Default::default()
            },
        );
        day.models.insert(
            "deepseek/deepseek-flash".into(),
            Bucket {
                requests: 1,
                input_tokens: 10,
                output_tokens: 5,
                duration_ms: 100,
                ..Default::default()
            },
        );
        stats.daily.insert(today(), day);

        let report = aggregate(&stats, 7);
        assert_eq!(report.total.requests, 2);
        assert_eq!(report.by_model.len(), 2);
        let kimi = report
            .by_model
            .iter()
            .find(|(name, _)| name == "kimi/kimi-k3")
            .expect("kimi 应出现在模型维度");
        assert_eq!(kimi.1.input_tokens, 100);
        assert_eq!(kimi.1.avg_ms(), 300);
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(n(0), "0");
        assert_eq!(n(999), "999");
        assert_eq!(n(1234), "1,234");
        assert_eq!(n(1234567), "1,234,567");
    }
}
