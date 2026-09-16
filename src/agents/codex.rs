//! Codex: write the Floway provider into `~/.codex/config.toml` and stage the
//! API key as the provider's command-auth token (`~/.codex/floway-token`).
//!
//! The managed keys mirror the Floway installer's `config/batchWrite` edits
//! (model_provider, model_providers.floway.*, features, model, and
//! model_reasoning_effort), but are applied directly to config.toml with
//! `toml_edit` so unconfigure can remove them again without driving the
//! `codex app-server`. Preserves unrelated config, comments, and formatting.
//!
//! Refs:
//!   https://github.com/openai/codex/blob/main/docs/config.md
//!   https://github.com/openai/codex/blob/main/codex-rs/model-provider-info/src/lib.rs

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::gateway::{Client, ModelList};

pub fn codex_home() -> PathBuf {
    if let Ok(dir) = std::env::var("CODEX_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = crate::fs_util::home_dir();
    PathBuf::from(home).join(".codex")
}

pub fn config_path() -> PathBuf {
    codex_home().join("config.toml")
}

pub fn token_path() -> PathBuf {
    codex_home().join("floway-token")
}

pub fn apply(client: &Client, models: &ModelList) -> Result<String> {
    let _ = models; // Codex refreshes its own model catalog online (command auth).
    let path = config_path();

    let mut doc = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        raw.parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("{path:?} is not valid TOML; leaving it untouched"))?
    } else {
        toml_edit::DocumentMut::new()
    };

    let root = doc.as_table_mut();
    root["model_provider"] = toml_edit::value("floway");
    root["suppress_unstable_features_warning"] = toml_edit::value(true);

    let providers = root
        .entry("model_providers")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .context("model_providers is not a TOML table; leaving config.toml untouched")?;
    let floway = providers
        .entry("floway")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .context("model_providers.floway is not a TOML table; leaving config.toml untouched")?;
    floway["name"] = toml_edit::value("Floway");
    floway["base_url"] = toml_edit::value(format!("{}/azure-api.codex", client.endpoint()));
    // Command auth opts the provider into online model refresh; the actor
    // marker enables Codex's client-owned search and image extensions.
    #[cfg(windows)]
    {
        let mut auth = toml_edit::InlineTable::new();
        auth.insert("command", "powershell".into());
        let mut args = toml_edit::Array::new();
        args.push("-NoProfile");
        args.push("-Command");
        args.push(r#"$h = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $HOME '.codex' }; [IO.File]::ReadAllText((Join-Path $h 'floway-token'))"#);
        auth.insert("args", toml_edit::Value::Array(args));
        floway["auth"] = toml_edit::value(toml_edit::Value::InlineTable(auth));
    }
    #[cfg(not(windows))]
    {
        let mut auth = toml_edit::InlineTable::new();
        auth.insert("command", "sh".into());
        let mut args = toml_edit::Array::new();
        args.push("-c");
        args.push(r#"cat "${CODEX_HOME:-$HOME/.codex}/floway-token""#);
        auth.insert("args", toml_edit::Value::Array(args));
        floway["auth"] = toml_edit::value(toml_edit::Value::InlineTable(auth));
    }
    floway["wire_api"] = toml_edit::value("responses");
    floway["supports_websockets"] = toml_edit::value(true);
    let mut headers = toml_edit::Table::new();
    headers.insert("x-openai-actor-authorization", toml_edit::value("1"));
    floway["http_headers"] = toml_edit::Item::Table(headers);

    let features = root
        .entry("features")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .context("features is not a TOML table; leaving config.toml untouched")?;
    features["apps"] = toml_edit::value(false);
    features["standalone_web_search"] = toml_edit::value(true);

    crate::toml_doc::save(&path, &doc)?;

    // Stage the provider token (mode 0600, atomic rename), matching the
    // installer's codex_stage_token.
    let token_path = token_path();
    if let Some(parent) = token_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::write_private_file(&token_path, client.api_key())?;

    Ok(format!(
        "wrote {} (provider `floway`) and the token at {}",
        path.display(),
        token_path.display()
    ))
}

pub fn unconfigure() -> Result<Option<String>> {
    let path = config_path();
    let token_path = token_path();
    let mut touched = Vec::new();

    if path.exists() {
        let raw = std::fs::read_to_string(&path)?;
        if let Ok(mut doc) = raw.parse::<toml_edit::DocumentMut>() {
            let root = doc.as_table_mut();
            let mut removed_any = false;

            if root.get("model_provider").and_then(|v| v.as_str()) == Some("floway") {
                root.remove("model_provider");
                removed_any = true;
            }
            if root.remove("suppress_unstable_features_warning").is_some() {
                removed_any = true;
            }

            let should_prune_providers = if let Some(providers) = root
                .get_mut("model_providers")
                .and_then(|i| i.as_table_like_mut())
            {
                if providers.remove("floway").is_some() {
                    removed_any = true;
                }
                providers.is_empty()
            } else {
                false
            };
            if should_prune_providers {
                root.remove("model_providers");
            }

            let should_prune_features = if let Some(features) =
                root.get_mut("features").and_then(|i| i.as_table_like_mut())
            {
                if features.remove("apps").is_some() {
                    removed_any = true;
                }
                if features.remove("standalone_web_search").is_some() {
                    removed_any = true;
                }
                features.is_empty()
            } else {
                false
            };
            if should_prune_features {
                root.remove("features");
            }

            if removed_any {
                // The file only existed for Floway's keys; an emptied
                // document is noise, so drop it.
                if doc.to_string().trim().is_empty() {
                    std::fs::remove_file(&path)?;
                } else {
                    crate::toml_doc::save(&path, &doc)?;
                }
                touched.push(path.display().to_string());
            }
            // An unparseable config.toml is user data floway never wrote.
        }
    }

    if token_path.exists() {
        std::fs::remove_file(&token_path)
            .with_context(|| format!("could not remove {}", token_path.display()))?;
        touched.push(token_path.display().to_string());
    }

    if touched.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("removed {}", touched.join(", "))))
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn unconfigure_preserves_unrelated_keys_and_comments() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-codex-test-1-{}", std::process::id()));
        let codex_dir = dir.join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::env::set_var("CODEX_HOME", &codex_dir);

        let client = crate::gateway::Client::new("http://gw".into(), "key".into()).unwrap();
        let models = crate::gateway::ModelList { data: vec![] };
        apply(&client, &models).unwrap();

        let path = config_path();
        let token = token_path();
        assert!(path.exists());
        assert!(token.exists());

        // Verify that apply wrote a relocatable auth command, not an absolute path
        let initial_content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !initial_content.contains(&dir.to_string_lossy().into_owned()),
            "config.toml should not contain absolute paths: {initial_content}"
        );
        assert!(initial_content.contains(r#"cat "${CODEX_HOME:-$HOME/.codex}/floway-token""#));

        // Hand-edit config.toml to add [model_providers.other] with a key and a [features] flag, plus a comment
        let user_additions = r#"
# User comment that must survive
[model_providers.other]
name = "Other Provider"
"#;
        let mut modified =
            initial_content.replace("[features]\n", "[features]\nmy_custom_feature = true\n");
        modified.push_str(user_additions);
        std::fs::write(&path, modified).unwrap();

        let res = unconfigure().unwrap();
        assert!(res.is_some(), "unconfigure should report touched files");

        // The file must survive because it has user config
        assert!(path.exists(), "config.toml should still exist");
        let content = std::fs::read_to_string(&path).unwrap();

        // Foreign config and comments survive
        assert!(content.contains("# User comment that must survive"));
        assert!(
            content.contains("model_providers.other")
                || content.contains("[model_providers.other]")
        );
        assert!(content.contains("Other Provider"));
        assert!(content.contains("my_custom_feature = true"));

        // Managed keys and tables are gone
        assert!(!content.contains("floway"));
        assert!(!content.contains("model_provider ="));
        assert!(!content.contains("suppress_unstable_features_warning"));
        assert!(!content.contains("standalone_web_search"));
        assert!(!content.contains("apps = false"));

        // floway-token is gone
        assert!(!token.exists(), "floway-token should be removed");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unconfigure_removes_floway_when_model_provider_switched() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-codex-test-2-{}", std::process::id()));
        let codex_dir = dir.join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::env::set_var("CODEX_HOME", &codex_dir);

        let client = crate::gateway::Client::new("http://gw".into(), "key".into()).unwrap();
        let models = crate::gateway::ModelList { data: vec![] };
        apply(&client, &models).unwrap();

        let path = config_path();
        let token = token_path();

        // Reinstall/hand-edit: set model_provider = "openai" by hand
        let initial_content = std::fs::read_to_string(&path).unwrap();
        let modified = initial_content.replace(
            r#"model_provider = "floway""#,
            r#"model_provider = "openai""#,
        );
        std::fs::write(&path, modified).unwrap();

        let res = unconfigure().unwrap();
        assert!(res.is_some(), "unconfigure should succeed");

        assert!(
            path.exists(),
            "config.toml should survive because model_provider = openai was preserved"
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"model_provider = "openai""#));
        assert!(!content.contains("floway"));
        assert!(!content.contains("model_providers"));
        assert!(!content.contains("features"));
        assert!(!token.exists(), "floway-token should be removed");

        std::fs::remove_dir_all(&dir).ok();
    }
}
