//! Claude Code: merge Floway settings into `~/.claude/settings.json`.
//!
//! Managed keys mirror the Floway bash installer's CLAUDE_MERGE_PROGRAM:
//! `.env.ANTHROPIC_BASE_URL`, `.env.ANTHROPIC_AUTH_TOKEN`, the five model
//! overrides, `.effortLevel`, and (by default, for discovery) the
//! `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY` flag. Uninstalling deletes
//! exactly those keys and leaves the rest of the document untouched.
//!
//! Refs:
//!   https://docs.claude.com/en/docs/claude-code/env-vars
//!   https://docs.claude.com/en/docs/claude-code/model-config#environment-variables
//!   https://code.claude.com/docs/en/settings

use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::gateway::{Client, ModelList};
use crate::json_doc;

const MANAGED_ENV_KEYS: [&str; 8] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
];

pub fn settings_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("settings.json");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".claude").join("settings.json")
}

pub fn apply(client: &Client, _models: &ModelList) -> Result<String> {
    let path = settings_path();
    let mut doc = json_doc::load_or_new(&path, "Claude settings")?;
    let api_key = client.api_key();

    let root = json_doc::ensure_object(&mut doc, "root")?;
    let env = json_doc::ensure_object_in(root, "env")?;
    env.insert("ANTHROPIC_BASE_URL".into(), json!(client.endpoint()));
    env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(api_key));
    // Floway's Claude Code discovery flag: the installer enables it by default
    // so `/v1/models` drives Claude's picker.
    env.insert(
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
        json!("1"),
    );

    let summary = format!("wrote {}", path.display());
    json_doc::save(&path, &doc, 0o600)?;
    Ok(summary)
}

pub fn unconfigure() -> Result<Option<String>> {
    let path = settings_path();
    if !path.exists() {
        return Ok(None);
    }
    let doc = json_doc::load_or_new(&path, "Claude settings")?;
    if !doc.is_object() {
        return Ok(None);
    }
    let mut doc = doc;
    let mut removed = 0usize;

    if let Some(env) = doc.get_mut("env").and_then(Value::as_object_mut) {
        for key in MANAGED_ENV_KEYS {
            if env.remove(key).is_some() {
                removed += 1;
            }
        }
        if env.is_empty() {
            doc.as_object_mut().unwrap().remove("env");
        }
    }

    if removed == 0 {
        return Ok(None);
    }
    json_doc::save(&path, &doc, 0o600)?;
    Ok(Some(format!(
        "removed {removed} managed keys from {}",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn apply_then_unconfigure_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-claude-e2e-{}", std::process::id()));
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::set_var("HOME", &dir);
        std::fs::create_dir_all(dir.join(".claude")).unwrap();

        let client = crate::gateway::Client::new("http://gw".into(), "key".into()).unwrap();
        let models = crate::gateway::ModelList { data: vec![] };
        apply(&client, &models).unwrap();

        // A foreign key the user set must survive.
        let path = settings_path();
        let mut doc: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        doc.as_object_mut()
            .unwrap()
            .insert("model".into(), json!("keep-me"));
        std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();

        let result = unconfigure().unwrap();
        assert!(result.is_some(), "unconfigure returned None after apply");
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            after.get("env").is_none(),
            "env block should be gone: {after}"
        );
        assert_eq!(after.get("model").and_then(Value::as_str), Some("keep-me"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
