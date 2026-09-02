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
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
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
    floway["auth"] = toml_edit::value(format!("cat \"{}\"", token_path().display()));
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
            let had_marker = root.get("model_provider").and_then(|v| v.as_str()) == Some("floway");
            root.remove("model_provider");
            root.remove("suppress_unstable_features_warning");
            root.remove("model_providers");
            root.remove("features");
            if had_marker {
                crate::toml_doc::save(&path, &doc)?;
                // The file only existed for Floway's keys; an emptied
                // document is noise, so drop it.
                if doc.to_string().trim().is_empty() {
                    std::fs::remove_file(&path)?;
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
