use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::fsutil;

/// 由 `ccms on` 独占管理的 env 键，`ccms off` 会精确移除这些。
const MANAGED_ENV: [&str; 12] = [
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
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
];

/// `ccms on` 前的受管状态。env 里只保存原本存在的键，缺失即删除；
/// model 用 has_model 区分“原本不存在”和“原值为 null”。
#[derive(Serialize, Deserialize, Default)]
struct PreviousState {
    #[serde(default)]
    env: BTreeMap<String, Value>,
    #[serde(default)]
    has_model: bool,
    #[serde(default)]
    model: Value,
}

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

fn backup(path: &Path) -> Result<()> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| format!("读取 {} 以创建备份", path.display()));
        }
    };
    let backup = Path::new(&format!("{}.ccms.bak", path.display())).to_path_buf();
    fsutil::write_atomic(&backup, &bytes, 0o600)
        .with_context(|| format!("备份 {} 到 {} 失败", path.display(), backup.display()))
}

fn validate_settings(path: &Path, root: &Value) -> Result<()> {
    let Some(object) = root.as_object() else {
        bail!("{} 顶层不是 JSON 对象", path.display());
    };
    if let Some(env) = object.get("env") {
        if !env.is_object() {
            bail!("{} 的 env 不是 JSON 对象，拒绝覆盖", path.display());
        }
    }
    Ok(())
}

fn read_previous(path: &Path) -> Result<Option<PreviousState>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} 已损坏，请先修复或删除它", path.display()))
            .map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("读取 {}", path.display())),
    }
}

fn write_previous(path: &Path, state: &PreviousState) -> Result<()> {
    let mut body = serde_json::to_vec_pretty(state).context("序列化 settings 前状态失败")?;
    body.push(b'\n');
    fsutil::write_atomic(path, &body, 0o600)
        .with_context(|| format!("保存 settings 前状态到 {} 失败", path.display()))
}

/// 识别 ccms / ccx 自己写入的路由状态，避免旧升级用户首次重复 on 时把
/// router token 当成“官方原值”保存下来。
fn is_router_model(model: &str) -> bool {
    model.starts_with("ccms-") || model.starts_with("ccx-")
}

fn looks_like_router_state(root: &Value) -> bool {
    let Some(env) = root.get("env").and_then(Value::as_object) else {
        return false;
    };
    let base = env
        .get("ANTHROPIC_BASE_URL")
        .and_then(Value::as_str)
        .unwrap_or("");
    let local = base.starts_with("http://127.0.0.1:")
        || base.starts_with("http://localhost:")
        || base.starts_with("http://[::1]:");
    let has_token = env
        .get("ANTHROPIC_AUTH_TOKEN")
        .and_then(Value::as_str)
        .is_some();
    let has_marker = env
        .get("ANTHROPIC_MODEL")
        .and_then(Value::as_str)
        .is_some_and(is_router_model)
        || env
            .get("ANTHROPIC_SMALL_FAST_MODEL")
            .and_then(Value::as_str)
            .is_some_and(is_router_model);
    local && has_token && has_marker
}

fn snapshot_previous(root: &Value) -> PreviousState {
    let object = root.as_object().expect("调用前已校验");
    let mut state = PreviousState::default();
    if let Some(env) = object.get("env").and_then(Value::as_object) {
        for key in MANAGED_ENV {
            if let Some(value) = env.get(key) {
                state.env.insert(key.to_string(), value.clone());
            }
        }
    }
    if let Some(model) = object.get("model") {
        state.has_model = true;
        state.model = model.clone();
    }
    state
}

fn remove_managed_env(object: &mut serde_json::Map<String, Value>) {
    let drop_env = if let Some(Value::Object(env)) = object.get_mut("env") {
        for key in MANAGED_ENV {
            env.remove(key);
        }
        env.is_empty()
    } else {
        false
    };
    if drop_env {
        object.remove("env");
    }
}

/// 写入路由模式的环境变量；其余设置（hooks、permissions 等）原样保留。
pub fn enable(
    path: &Path,
    previous: &Path,
    base: &str,
    effort: &str,
    default_tier: &str,
    token: &str,
) -> Result<()> {
    let mut root = read(path)?;
    validate_settings(path, &root)?;
    backup(path)?;

    let previous_state = read_previous(previous)?;
    let created_previous = previous_state.is_none() && !looks_like_router_state(&root);
    if created_previous {
        // 先保存前状态；写失败时 settings 保持原样。
        write_previous(previous, &snapshot_previous(&root))?;
    }

    let object = root.as_object_mut().expect("validate_settings 已检查");
    let env = object
        .entry("env")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .expect("validate_settings 已检查 env 对象");

    env.remove("ANTHROPIC_API_KEY");
    env.insert("ANTHROPIC_BASE_URL".into(), Value::String(base.to_string()));
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".into(),
        Value::String(token.to_string()),
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
        object.insert(
            "model".into(),
            Value::String(format!("ccms-{default_tier}")),
        );
    }

    match write(path, &root) {
        Ok(()) => Ok(()),
        Err(err) if created_previous => {
            // 只回滚本次新建的前状态；已有 previous 的失败绝不能删除它。
            match fs::remove_file(previous) {
                Ok(()) => {}
                Err(remove_err) if remove_err.kind() == std::io::ErrorKind::NotFound => {}
                Err(remove_err) => {
                    return Err(anyhow!(
                        "写入 {} 失败，且回滚本次新建的 {} 也失败: {err:#}; {remove_err}",
                        path.display(),
                        previous.display()
                    ));
                }
            }
            Err(err)
        }
        Err(err) => Err(err),
    }
}

pub fn disable(path: &Path, previous: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut root = read(path)?;
    validate_settings(path, &root)?;
    backup(path)?;

    let previous_state = read_previous(previous)?;
    let had_previous = previous_state.is_some();
    let object = root.as_object_mut().expect("validate_settings 已检查");
    remove_managed_env(object);

    if let Some(state) = previous_state {
        if !state.env.is_empty() {
            let env = object
                .entry("env")
                .or_insert_with(|| Value::Object(Default::default()))
                .as_object_mut()
                .expect("新建 env 一定是对象");
            for (key, value) in state.env {
                env.insert(key, value);
            }
        }
        if state.has_model {
            object.insert("model".into(), state.model);
        } else {
            object.remove("model");
        }
    }

    write(path, &root)?;
    if had_previous {
        match fs::remove_file(previous) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("settings 已恢复，但删除 {} 失败", previous.display())
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ccms-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(Path::new(&format!("{}.ccms.bak", path.display())));
        let _ = fs::remove_file(Path::new(&format!("{}.ccms.bak", path.display())));
        path
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn previous_file(path: &Path) -> std::path::PathBuf {
        let previous = path.with_extension("previous.json");
        let _ = fs::remove_file(&previous);
        previous
    }

    #[test]
    fn enable_then_disable_restores_official_key_and_model() {
        let path = temp_file("restore.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"model":"claude-original","hooks":{"SessionStart":[]},"env":{"ANTHROPIC_API_KEY":"sk-dummy-official","FOO":"keep"}}"#,
        )
        .unwrap();

        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        let routed = read_json(&path);
        assert!(routed["env"].get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(
            routed["env"]["ANTHROPIC_AUTH_TOKEN"],
            json!("random-router-token-abcdefghijklmnopqrstuvwxyz")
        );

        disable(&path, &previous).unwrap();
        let restored = read_json(&path);
        assert_eq!(
            restored["env"]["ANTHROPIC_API_KEY"],
            json!("sk-dummy-official"),
            "off 必须恢复官方 API key"
        );
        assert_eq!(restored["env"]["FOO"], json!("keep"));
        assert_eq!(restored["hooks"]["SessionStart"], json!([]));
        assert_eq!(restored["model"], json!("claude-original"));
        assert!(
            restored["env"].get("ANTHROPIC_BASE_URL").is_none(),
            "ccms 受管值必须移除"
        );
        assert!(!previous.exists(), "成功恢复后必须删除前状态文件");
    }

    #[cfg(unix)]
    #[test]
    fn saved_previous_state_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_file("previous-mode.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_API_KEY":"sk-dummy-official"}}"#,
        )
        .unwrap();

        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        let mode = fs::metadata(&previous).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "settings.previous.json 必须是 600");
    }

    #[test]
    fn repeated_enable_does_not_replace_saved_previous_state() {
        let path = temp_file("repeat-on.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_API_KEY":"sk-dummy-official","FOO":"keep"}}"#,
        )
        .unwrap();

        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "first-random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "low",
            "opus",
            "second-random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        disable(&path, &previous).unwrap();

        let restored = read_json(&path);
        assert_eq!(
            restored["env"]["ANTHROPIC_API_KEY"],
            json!("sk-dummy-official"),
            "重复 on 不得把 router token 当前状态当成原状态"
        );
        assert!(
            restored["env"].get("ANTHROPIC_AUTH_TOKEN").is_none(),
            "router token 必须被移除而不是恢复"
        );
    }

    #[test]
    fn disable_preserves_unmanaged_edits_made_while_routed() {
        let path = temp_file("unmanaged.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"permissions":{"allow":["Read"]},"env":{"ANTHROPIC_API_KEY":"sk-dummy-official","FOO":"keep"}}"#,
        )
        .unwrap();
        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();

        let mut edited = read_json(&path);
        edited["env"]["EDITED_WHILE_ROUTED"] = json!("yes");
        edited["hooks"] = json!({"Stop": [{"type": "command"}]});
        edited["permissions"]["allow"]
            .as_array_mut()
            .unwrap()
            .push(json!("Write"));
        fs::write(&path, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();

        disable(&path, &previous).unwrap();
        let restored = read_json(&path);
        assert_eq!(restored["env"]["EDITED_WHILE_ROUTED"], json!("yes"));
        assert_eq!(
            restored["env"]["ANTHROPIC_API_KEY"],
            json!("sk-dummy-official")
        );
        assert_eq!(restored["hooks"]["Stop"][0]["type"], json!("command"));
        assert_eq!(restored["permissions"]["allow"], json!(["Read", "Write"]));
    }

    #[test]
    fn disable_without_previous_state_only_removes_managed_values() {
        let path = temp_file("old-upgrade.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_API_KEY":"sk-not-real","ANTHROPIC_BASE_URL":"http://127.0.0.1:8788","ANTHROPIC_AUTH_TOKEN":"router-token","ANTHROPIC_MODEL":"ccms-sonnet","FOO":"keep"}}"#,
        )
        .unwrap();

        disable(&path, &previous).unwrap();
        let restored = read_json(&path);
        assert_eq!(restored["env"], json!({"FOO":"keep"}));
        assert!(
            restored["env"].get("ANTHROPIC_API_KEY").is_none(),
            "没有前状态时不得虚构官方 key"
        );
    }

    #[test]
    fn enabling_an_old_router_state_does_not_save_it_as_previous() {
        let path = temp_file("old-upgrade-enable.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8788","ANTHROPIC_AUTH_TOKEN":"old-router-token","ANTHROPIC_MODEL":"ccms-sonnet","FOO":"keep"}}"#,
        )
        .unwrap();

        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "new-random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        assert!(!previous.exists(), "旧 router 状态不能被当作官方原值保存");

        disable(&path, &previous).unwrap();
        let restored = read_json(&path);
        assert_eq!(restored["env"], json!({"FOO":"keep"}));
    }

    #[test]
    fn enable_rejects_non_object_env_without_touching_bytes() {
        let path = temp_file("invalid-env.json");
        let previous = previous_file(&path);
        let original = br#"{"env":"not-an-object","hooks":{}}"#.to_vec();
        fs::write(&path, &original).unwrap();

        let err = enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .expect_err("非对象 env 必须明确报错");
        assert!(
            format!("{err:#}").contains("env"),
            "错误应指出 env: {err:#}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "失败时原文件字节必须不变"
        );
    }

    #[test]
    fn backup_failure_aborts_enable_without_touching_settings() {
        let path = temp_file("backup-enable.json");
        let previous = previous_file(&path);
        let original = br#"{"env":{"ANTHROPIC_API_KEY":"sk-dummy","FOO":"keep"}}"#.to_vec();
        fs::write(&path, &original).unwrap();
        let backup = path.with_extension("json.ccms.bak");
        fs::create_dir(backup).unwrap();

        let err = enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .expect_err("备份失败必须阻止写入");
        assert!(
            format!("{err:#}").contains("备份"),
            "错误应说明备份失败: {err:#}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "备份失败不得改 settings"
        );
    }

    #[test]
    fn backup_failure_aborts_disable_without_touching_settings() {
        let path = temp_file("backup-disable.json");
        let previous = previous_file(&path);
        let original =
            br#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8788","FOO":"keep"}}"#.to_vec();
        fs::write(&path, &original).unwrap();
        let backup = path.with_extension("json.ccms.bak");
        fs::create_dir(backup).unwrap();

        let err = disable(&path, &previous).expect_err("备份失败必须阻止 disable");
        assert!(
            format!("{err:#}").contains("备份"),
            "错误应说明备份失败: {err:#}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "备份失败不得改 settings"
        );
    }

    #[test]
    fn failed_first_enable_removes_new_previous_and_retry_snapshots_latest_values() {
        let path = temp_file("rollback-previous.json");
        let previous = previous_file(&path);
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::write(
            &path,
            r#"{"model":"claude-first","env":{"ANTHROPIC_API_KEY":"sk-dummy-first","FOO":"keep"}}"#,
        )
        .unwrap();

        // 目录占据 settings 原子写的临时路径，使最终写入必然失败。
        fs::create_dir(&tmp).unwrap();
        let err = enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "first-random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .expect_err("settings 写入失败必须报错");
        assert!(
            !previous.exists(),
            "本次 enable 新建的 previous 必须在写入失败时回滚: {err:#}"
        );
        let unchanged = read_json(&path);
        assert_eq!(
            unchanged["env"]["ANTHROPIC_API_KEY"],
            json!("sk-dummy-first")
        );
        assert_eq!(unchanged["model"], json!("claude-first"));

        // 移除故障并修改官方受管值/model 后重试：必须快照第二次的值。
        fs::remove_dir(&tmp).unwrap();
        fs::write(
            &path,
            r#"{"model":"claude-second","env":{"ANTHROPIC_API_KEY":"sk-dummy-second","FOO":"keep"}}"#,
        )
        .unwrap();
        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "second-random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        assert!(previous.exists(), "重试成功必须保存前状态");

        disable(&path, &previous).unwrap();
        let restored = read_json(&path);
        assert_eq!(
            restored["env"]["ANTHROPIC_API_KEY"],
            json!("sk-dummy-second"),
            "off 必须恢复第二次修改后的官方值，而不是第一次的过期快照"
        );
        assert_eq!(restored["model"], json!("claude-second"));
    }

    #[test]
    fn failed_enable_never_deletes_existing_previous() {
        let path = temp_file("keep-existing-previous.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_API_KEY":"sk-dummy-official","FOO":"keep"}}"#,
        )
        .unwrap();
        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "first-random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        let before = fs::read(&previous).unwrap();

        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir(&tmp).unwrap();
        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "low",
            "opus",
            "second-random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .expect_err("settings 写入失败必须报错");

        assert_eq!(
            fs::read(&previous).unwrap(),
            before,
            "已有 previous 在写入失败时绝不能被删除或改写"
        );
        fs::remove_dir(&tmp).unwrap();
    }

    #[test]
    fn enable_preserves_unmanaged_settings() {
        let path = temp_file("settings.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"hooks":{"SessionStart":[]},"env":{"FOO":"1"},"model":""}"#,
        )
        .unwrap();
        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "test-router-token",
        )
        .unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["hooks"]["SessionStart"], json!([]));
        assert_eq!(value["env"]["FOO"], json!("1"));
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], json!("ccms-sonnet"));
        assert_eq!(
            value["env"]["ANTHROPIC_AUTH_TOKEN"],
            json!("test-router-token")
        );
        assert_eq!(
            value["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
            json!("ccms-subagent")
        );
        assert_eq!(value["model"], json!("ccms-sonnet"));
        assert!(path.with_extension("json.ccms.bak").exists());
    }

    #[test]
    fn disable_removes_only_managed_env() {
        let path = temp_file("settings-off.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"env":{"FOO":"1","ANTHROPIC_BASE_URL":"http://127.0.0.1:8788","ANTHROPIC_MODEL":"ccms-sonnet"}}"#,
        )
        .unwrap();
        disable(&path, &previous).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["env"], json!({"FOO":"1"}));

        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8788"}}"#,
        )
        .unwrap();
        disable(&path, &previous).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value.get("env").is_none(), "空 env 应整体删除");
    }

    #[test]
    fn top_level_model_follows_default_tier_and_off_restores_original() {
        let path = temp_file("model-tier.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"model":"haiku","env":{"ANTHROPIC_API_KEY":"sk-dummy-official"}}"#,
        )
        .unwrap();

        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "haiku",
            "random-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        let routed = read_json(&path);
        assert_eq!(
            routed["model"],
            json!("ccms-haiku"),
            "顶层 model 必须与 ANTHROPIC_MODEL 一样写 ccms-{{default_tier}}"
        );
        assert_eq!(routed["env"]["ANTHROPIC_MODEL"], json!("ccms-haiku"));

        disable(&path, &previous).unwrap();
        let restored = read_json(&path);
        assert_eq!(
            restored["model"],
            json!("haiku"),
            "off 必须恢复原顶层 model"
        );
    }

    #[test]
    fn ccx_router_state_is_not_snapshotted_as_official_values() {
        let path = temp_file("ccx-migrate.json");
        let previous = previous_file(&path);
        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8788","ANTHROPIC_AUTH_TOKEN":"old-ccx-token","ANTHROPIC_MODEL":"ccx-sonnet","ANTHROPIC_SMALL_FAST_MODEL":"ccx-haiku","FOO":"keep"}}"#,
        )
        .unwrap();

        enable(
            &path,
            &previous,
            "http://127.0.0.1:8788",
            "high",
            "sonnet",
            "new-router-token-abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        assert!(
            !previous.exists(),
            "ccx 时代的路由 env 不得被当作官方原值保存"
        );
        let routed = read_json(&path);
        assert_eq!(
            routed["env"]["ANTHROPIC_AUTH_TOKEN"],
            json!("new-router-token-abcdefghijklmnopqrstuvwxyz")
        );

        disable(&path, &previous).unwrap();
        let restored = read_json(&path);
        assert!(
            restored["env"].get("ANTHROPIC_BASE_URL").is_none(),
            "off 不得恢复 ccx 时代的死端点"
        );
        assert!(
            restored["env"].get("ANTHROPIC_AUTH_TOKEN").is_none(),
            "off 不得恢复 ccx 时代的旧 token"
        );
        assert_eq!(restored["env"]["FOO"], json!("keep"));
    }
}
