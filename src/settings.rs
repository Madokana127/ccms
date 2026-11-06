use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use crate::fsutil;

/// 由 `ccms on` 独占管理的 env 键，`ccms off` 会精确移除这些。
const MANAGED_ENV: [&str; 11] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_EFFORT_LEVEL",
];

pub fn read(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) => {
            if text.trim().is_empty() {
                Ok(json!({}))
            } else {
                serde_json::from_str(&text)
                    .with_context(|| format!("{} 不是合法 JSON，请先修好它", path.display()))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(err) => Err(err).with_context(|| format!("读取 {}", path.display())),
    }
}

pub fn base_url(settings: &Value) -> Option<String> {
    settings
        .get("env")?
        .get("ANTHROPIC_BASE_URL")?
        .as_str()
        .map(|value| value.trim_end_matches('/').to_string())
}

fn write(path: &Path, root: &Value) -> Result<()> {
    let mut body = serde_json::to_vec_pretty(root)?;
    body.push(b'\n');
    fsutil::write_atomic(path, &body, 0o600)
}

fn backup(path: &Path) {
    if let Ok(bytes) = fs::read(path) {
        let backup = Path::new(&format!("{}.ccms.bak", path.display())).to_path_buf();
        let _ = fsutil::write_atomic(&backup, &bytes, 0o600);
    }
}

/// 写入路由模式的环境变量；其余设置（hooks、permissions 等）原样保留。
pub fn enable(path: &Path, base: &str, effort: &str, default_tier: &str) -> Result<()> {
    let mut root = read(path)?;
    let Some(object) = root.as_object_mut() else {
        bail!("{} 顶层不是 JSON 对象", path.display());
    };

    backup(path);

    let env = object
        .entry("env")
        .or_insert_with(|| Value::Object(Default::default()));
    if !env.is_object() {
        *env = Value::Object(Default::default());
    }
    let env = env.as_object_mut().expect("env 必须是对象");

    env.remove("ANTHROPIC_API_KEY");
    env.insert("ANTHROPIC_BASE_URL".into(), Value::String(base.to_string()));
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".into(),
        Value::String("ccms-router-local".into()),
    );
    env.insert(
        "ANTHROPIC_MODEL".into(),
        Value::String(format!("ccms-{default_tier}")),
    );
    env.insert(
        "ANTHROPIC_SMALL_FAST_MODEL".into(),
        Value::String("ccms-haiku".into()),
    );
    for tier in ["fable", "opus", "sonnet", "haiku"] {
        env.insert(
            format!("ANTHROPIC_DEFAULT_{}_MODEL", tier.to_uppercase()),
            Value::String(format!("ccms-{tier}")),
        );
    }
    env.insert(
        "CLAUDE_CODE_SUBAGENT_MODEL".into(),
        Value::String("ccms-subagent".into()),
    );
    if !effort.trim().is_empty() {
        env.insert(
            "CLAUDE_CODE_EFFORT_LEVEL".into(),
            Value::String(effort.to_string()),
        );
    }
    env.insert(
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
        Value::String("1".into()),
    );

    // 历史配置里 model 为空或 "haiku" 时，Claude Code 会忽略档位默认值。
    let needs_model = match object.get("model").and_then(|v| v.as_str()) {
        Some(model) => model.is_empty() || model == "haiku",
        None => false,
    };
    if needs_model {
        object.insert("model".into(), Value::String("sonnet".into()));
    }

    write(path, &root)
}

pub fn disable(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut root = read(path)?;
    let Some(object) = root.as_object_mut() else {
        bail!("{} 顶层不是 JSON 对象", path.display());
    };

    backup(path);

    let drop_env = if let Some(Value::Object(env)) = object.get_mut("env") {
        for key in MANAGED_ENV {
            env.remove(key);
        }
        env.remove("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC");
        env.is_empty()
    } else {
        false
    };
    if drop_env {
        object.remove("env");
    }

    write(path, &root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ccms-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn enable_preserves_unmanaged_settings() {
        let path = temp_file("settings.json");
        fs::write(
            &path,
            r#"{"hooks":{"SessionStart":[]},"env":{"FOO":"1"},"model":""}"#,
        )
        .unwrap();
        enable(&path, "http://127.0.0.1:8788", "high", "sonnet").unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["hooks"]["SessionStart"], json!([]));
        assert_eq!(value["env"]["FOO"], json!("1"));
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], json!("ccms-sonnet"));
        assert_eq!(
            value["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
            json!("ccms-subagent")
        );
        assert_eq!(value["model"], json!("sonnet"));
        assert!(path.with_extension("json.ccms.bak").exists());
    }

    #[test]
    fn disable_removes_only_managed_env() {
        let path = temp_file("settings-off.json");
        fs::write(
            &path,
            r#"{"env":{"FOO":"1","ANTHROPIC_BASE_URL":"http://127.0.0.1:8788","ANTHROPIC_MODEL":"ccms-sonnet"}}"#,
        )
        .unwrap();
        disable(&path).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["env"], json!({"FOO":"1"}));

        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8788"}}"#,
        )
        .unwrap();
        disable(&path).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value.get("env").is_none(), "空 env 应整体删除");
    }
}
