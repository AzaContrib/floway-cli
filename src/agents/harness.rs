//! Harness agents: oh-my-pi, Pi, opencode, Zed, VSCode, and DeepSeek Harness.
//! Rust ports of the Floway converters plus native merge/unmerge writers.
//!
//! Each writer touches only the `Floway` (or `floway`) provider subtree it
//! owns and leaves the rest of the document intact; unconfigure removes
//! exactly that subtree.

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::gateway::{Client, Model, ModelList, Rates};
use crate::json_doc;

const UPSTREAM: &str = "Floway";
const DEFAULT_CONTEXT_WINDOW: u64 = 262_144;
const DEFAULT_MAX_OUTPUT: u64 = 65_536;

// ---------------------------------------------------------------------------
// paths

/// Read a config-dir environment override: trimmed, with an empty value
/// treated as unset.
fn env_dir(name: &str) -> Option<PathBuf> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

/// Expand a leading `~`/`~/` the way Pi's `expandTildePath` does. oh-my-pi
/// resolves its override with `path.resolve`, which does *not* expand `~`, so
/// only the Pi writer applies this.
fn expand_tilde(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return crate::fs_util::home_dir();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return crate::fs_util::home_dir().join(rest);
    }
    path
}

pub fn omp_paths() -> (PathBuf, PathBuf) {
    // oh-my-pi derives its agent dir from `PI_CODING_AGENT_DIR` and its config
    // root from `PI_CONFIG_DIR` (a dirname joined onto `$HOME`, default
    // `.omp`). It never reads `OMP_CONFIG_DIR`.
    let dir = match env_dir("PI_CODING_AGENT_DIR") {
        Some(dir) => dir,
        None => {
            let home = crate::fs_util::home_dir();
            match env_dir("PI_CONFIG_DIR") {
                Some(root) => home.join(root).join("agent"),
                None => home.join(".omp").join("agent"),
            }
        }
    };
    (dir.join("models.yml"), dir.join(".env"))
}

/// `~/.pi/agent/models.json`, honoring Pi's only config-dir override.
/// Pi reads `PI_CODING_AGENT_DIR` (the env name it derives from its package
/// `piConfig.name`) and expands a leading `~` itself; `PI_CONFIG_DIR` is
/// oh-my-pi's variable and is deliberately not consulted here.
pub fn pi_path() -> PathBuf {
    if let Some(dir) = env_dir("PI_CODING_AGENT_DIR") {
        return expand_tilde(dir).join("models.json");
    }
    let home = crate::fs_util::home_dir();
    home.join(".pi").join("agent").join("models.json")
}

pub fn opencode_path() -> PathBuf {
    let dir = match std::env::var("OPENCODE_CONFIG_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = crate::fs_util::home_dir();
            home.join(".config").join("opencode")
        }
    };
    dir.join("opencode.json")
}

pub fn zed_path() -> PathBuf {
    if let Ok(dir) = std::env::var("ZED_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("global_settings.json");
        }
    }
    #[cfg(windows)]
    {
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| crate::fs_util::home_dir().join("AppData").join("Roaming"));
        appdata.join("Zed").join("global_settings.json")
    }
    #[cfg(not(windows))]
    {
        let home = crate::fs_util::home_dir();
        home.join(".config").join("zed").join("global_settings.json")
    }
}

pub fn vscode_path() -> PathBuf {
    if let Ok(dir) = std::env::var("VSCODE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("chatLanguageModels.json");
        }
    }
    let home = crate::fs_util::home_dir();
    #[cfg(windows)]
    let base = {
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData").join("Roaming"));
        appdata.join("Code").join("User")
    };
    #[cfg(target_os = "macos")]
    let base = home.join("Library/Application Support/Code/User");
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let base = home.join(".config/Code/User");
    base.join("chatLanguageModels.json")
}

pub fn dsh_paths() -> (PathBuf, PathBuf) {
    let dir = match std::env::var("DSH_CONFIG_DIR").or_else(|_| std::env::var("DSH_HOME")) {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = crate::fs_util::home_dir();
            home.join(".dsh")
        }
    };
    (dir.join("settings.yaml"), dir.join(".credentials.yaml"))
}

// ---------------------------------------------------------------------------
// shared conversion helpers (ports of the Python converters' model_config)

fn chat_models(models: &ModelList) -> Vec<&Model> {
    models.data.iter().filter(|m| m.is_chat()).collect()
}

/// The default (selector-less) pricing entry rates, as the converters pick.
fn default_rates(model: &Model) -> Option<&Rates> {
    Rates::default_entry(&model.pricing)
}

/// Decimal-string rate scaled by 1e6 into a per-million JSON number, matching
/// the converters' `Decimal(...).scaleb(6)` / `* 1e6` behavior.
fn rate_f64(value: &str) -> Option<f64> {
    Rates::scaleb6(value)
}

// ---------------------------------------------------------------------------
// oh-my-pi

pub fn apply_omp(client: &Client, models: &ModelList) -> Result<String> {
    let mut provider = serde_json::Map::new();
    provider.insert("baseUrl".into(), json!(format!("{}/v1", client.endpoint())));
    provider.insert("apiKey".into(), json!("FLOWAY_API_KEY"));
    provider.insert("api".into(), json!("openai-responses"));
    provider.insert(
        "models".into(),
        Value::Array(
            chat_models(models)
                .iter()
                .map(|m| omp_model_config(m))
                .collect(),
        ),
    );

    // The omp provider references the key by env name; the real token is
    // staged into the agent directory's .env, which oh-my-pi loads eagerly.
    let (models_path, env_path) = omp_paths();
    if let Some(parent) = models_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut doc = crate::yaml_doc::load_or_new(&models_path, "oh-my-pi models")?;
    let root = json_doc::ensure_object(&mut doc, "root")?;
    let providers = json_doc::ensure_object_in(root, "providers")?;
    providers.insert(UPSTREAM.into(), Value::Object(provider));

    let yaml = crate::yaml_doc::to_yaml(&doc)?;
    crate::write_private_file(&models_path, &yaml)?;

    // Preserve unrelated .env lines, replacing any prior FLOWAY_API_KEY entry.
    let mut lines: Vec<String> = if env_path.exists() {
        std::fs::read_to_string(&env_path)?
            .lines()
            .filter(|line| !line.starts_with("FLOWAY_API_KEY="))
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    let key = client.api_key();
    if key.contains('\'') || key.contains('\\') || key.contains('\n') {
        bail!("the API key contains a quote, backslash, or newline, which oh-my-pi's .env cannot carry; use a simpler key");
    }
    lines.push(format!("FLOWAY_API_KEY='{key}'"));
    let mut env_body = lines.join("\n");
    if !env_body.is_empty() {
        env_body.push('\n');
    }
    crate::write_private_file(&env_path, &env_body)?;

    Ok(format!(
        "wrote {} and the key in {}",
        models_path.display(),
        env_path.display()
    ))
}

fn omp_model_config(model: &Model) -> Value {
    let mut config = serde_json::Map::new();
    config.insert("id".into(), json!(model.id));
    config.insert(
        "name".into(),
        json!(model
            .display_name
            .clone()
            .unwrap_or_else(|| model.id.clone())),
    );
    if model.chat.reasoning.is_some() {
        config.insert("reasoning".into(), json!(true));
    }
    let input: Vec<&String> = model.chat.modalities.input.iter().collect();
    if !input.is_empty() {
        config.insert("input".into(), json!(model.chat.modalities.input));
    }
    if let Some(ctx) = model.limits.max_context_window_tokens {
        config.insert("contextWindow".into(), json!(ctx));
    }
    if let Some(out) = model.limits.max_output_tokens {
        config.insert("maxTokens".into(), json!(out));
    }
    if let Some(rates) = default_rates(model) {
        let input_tokens = rates.input_tokens.as_deref();
        let output_tokens = rates.output_tokens.as_deref();
        let cache_read = rates.input_cache_read_tokens.as_deref();
        let cache_write = rates.input_cache_write_tokens.as_deref();
        if input_tokens.is_some()
            || output_tokens.is_some()
            || cache_read.is_some()
            || cache_write.is_some()
        {
            let mut cost = serde_json::Map::new();
            cost.insert(
                "input".into(),
                json!(input_tokens.and_then(rate_f64).unwrap_or(0.0)),
            );
            cost.insert(
                "output".into(),
                json!(output_tokens.and_then(rate_f64).unwrap_or(0.0)),
            );
            cost.insert(
                "cacheRead".into(),
                json!(cache_read.and_then(rate_f64).unwrap_or(0.0)),
            );
            cost.insert(
                "cacheWrite".into(),
                json!(cache_write.and_then(rate_f64).unwrap_or(0.0)),
            );
            config.insert("cost".into(), Value::Object(cost));
        }
    }
    config.insert("compat".into(), json!({ "supportsStore": true }));
    Value::Object(config)
}


pub fn unconfigure_omp() -> Result<Option<String>> {
    let (models_path, env_path) = omp_paths();
    let mut touched = Vec::new();

    if models_path.exists() {
        let text = std::fs::read_to_string(&models_path)?;
        if let Ok(Value::Object(mut doc)) = crate::yaml_doc::from_yaml(&text) {
            if remove_provider(&mut doc, &["providers"]) {
                // A document that only held the Floway provider is removed
                // outright; otherwise the pruned document is written back.
                if doc.is_empty() {
                    std::fs::remove_file(&models_path)?;
                } else {
                    let body = crate::yaml_doc::to_yaml(&Value::Object(doc))?;
                    crate::write_private_file(&models_path, &body)?;
                }
                touched.push(models_path.display().to_string());
            }
        }
    }

    if env_path.exists() {
        let text = std::fs::read_to_string(&env_path)?;
        let kept: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with("FLOWAY_API_KEY="))
            .collect();
        if kept.len() != text.lines().count() {
            if kept.is_empty() {
                std::fs::remove_file(&env_path)?;
            } else {
                let mut body = kept.join("\n");
                body.push('\n');
                crate::write_private_file(&env_path, &body)?;
            }
            touched.push(env_path.display().to_string());
        }
    }

    if touched.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("removed Floway from {}", touched.join(", "))))
}

// ---------------------------------------------------------------------------
// Pi

/// Escape a literal for Pi's config-value syntax, where `$VAR`/`${VAR}`
/// interpolate, `!cmd` executes a shell command, `$$` emits `$`, and `$!`
/// emits `!`. Without this, a custom API key containing `$` reads as a missing
/// env var (Pi then reports the provider as unauthenticated) and a key
/// starting with `!` would be run as a command.
fn pi_config_value(literal: &str) -> String {
    let mut escaped = literal.replace('$', "$$");
    if escaped.starts_with('!') {
        escaped.replace_range(..1, "$!");
    }
    escaped
}

/// A Pi model entry. Pi validates strictly (`name`/`id` need at least one
/// character) and drops *every* provider in the file on any schema error, so
/// blank display names fall back to the model id and nameless ids are skipped.
fn pi_model_config(model: &Model) -> Option<Value> {
    if model.id.is_empty() {
        return None;
    }
    let mut config = serde_json::Map::new();
    config.insert("id".into(), json!(model.id));
    let name = model
        .display_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&model.id);
    config.insert("name".into(), json!(name));
    if model.chat.reasoning.is_some() {
        config.insert("reasoning".into(), json!(true));
    }
    let input: Vec<&String> = model.chat.modalities.input.iter().collect();
    if !input.is_empty() {
        config.insert("input".into(), json!(model.chat.modalities.input));
    }
    if let Some(ctx) = model.limits.max_context_window_tokens {
        config.insert("contextWindow".into(), json!(ctx));
    }
    if let Some(out) = model.limits.max_output_tokens {
        config.insert("maxTokens".into(), json!(out));
    }
    if let Some(rates) = default_rates(model) {
        let mut cost = serde_json::Map::new();
        for (key, value) in [
            ("input", rates.input_tokens.as_deref()),
            ("output", rates.output_tokens.as_deref()),
            ("cacheRead", rates.input_cache_read_tokens.as_deref()),
            ("cacheWrite", rates.input_cache_write_tokens.as_deref()),
        ] {
            cost.insert(key.into(), json!(value.and_then(rate_f64).unwrap_or(0.0)));
        }
        config.insert("cost".into(), Value::Object(cost));
    }
    Some(Value::Object(config))
}

pub fn apply_pi(client: &Client, models: &ModelList) -> Result<String> {
    let path = pi_path();
    // Pi accepts the JSONC superset (comments, trailing commas, BOM); read the
    // same superset so a file Pi can load is never rejected here.
    let mut doc = json_doc::load_or_new_jsonc(&path, "Pi models")?;
    // Pi requires the `providers` key to be present at the top level.
    let root = json_doc::ensure_object(&mut doc, "root")?;
    let providers = json_doc::ensure_object_in(root, "providers")?;

    let mut provider = serde_json::Map::new();
    provider.insert("baseUrl".into(), json!(format!("{}/v1", client.endpoint())));
    provider.insert("apiKey".into(), json!(pi_config_value(client.api_key())));
    provider.insert("api".into(), json!("openai-responses"));
    provider.insert(
        "models".into(),
        Value::Array(
            chat_models(models)
                .iter()
                .filter_map(|m| pi_model_config(m))
                .collect(),
        ),
    );

    providers.insert(UPSTREAM.into(), Value::Object(provider));

    json_doc::save(&path, &doc, 0o600)?;
    Ok(format!("wrote {}", path.display()))
}

pub fn unconfigure_pi() -> Result<Option<String>> {
    let path = pi_path();
    if !path.exists() {
        return Ok(None);
    }
    // Unlike a foreign config file floway never wrote, this one carries the
    // API key, so an unreadable document must surface as a failure rather than
    // "nothing to remove" — that would strand the key and drop the record.
    let mut doc = json_doc::load_or_new_jsonc(&path, "Pi models")?;
    if !doc.is_object() {
        return Ok(None);
    }
    let root = doc.as_object_mut().unwrap();
    if !remove_provider(root, &["providers"]) {
        return Ok(None);
    }
    // A document that only held the Floway provider is removed outright.
    // Anything else keeps a `providers` key: Pi rejects the whole file when it
    // is absent, which would take the user's other providers down with it.
    if root.is_empty() {
        std::fs::remove_file(&path)?;
    } else {
        root.entry("providers")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        json_doc::save(&path, &doc, 0o600)?;
    }
    Ok(Some(format!("removed Floway from {}", path.display())))
}

// ---------------------------------------------------------------------------
// opencode

pub fn apply_opencode(client: &Client, models: &ModelList) -> Result<String> {
    let path = opencode_path();
    let mut doc = json_doc::load_or_new(&path, "opencode config")?;
    let root = json_doc::ensure_object(&mut doc, "root")?;
    let providers = json_doc::ensure_object_in(root, "provider")?;
    let mut provider = serde_json::Map::new();
    provider.insert("name".into(), json!(UPSTREAM));
    provider.insert("npm".into(), json!("@ai-sdk/openai-compatible"));
    provider.insert(
        "options".into(),
        json!({
            "baseURL": format!("{}/v1", client.endpoint()),
            "setCacheKey": true,
            "apiKey": client.api_key(),
        }),
    );
    let mut model_map = serde_json::Map::new();
    for model in chat_models(models) {
        model_map.insert(model.id.clone(), opencode_model_config(model));
    }
    provider.insert("models".into(), Value::Object(model_map));
    providers.insert(UPSTREAM.into(), Value::Object(provider));

    json_doc::save(&path, &doc, 0o600)?;
    Ok(format!("wrote {}", path.display()))
}

fn opencode_model_config(model: &Model) -> Value {
    let mut config = serde_json::Map::new();
    config.insert("id".into(), json!(model.id));
    config.insert(
        "name".into(),
        json!(model
            .display_name
            .clone()
            .unwrap_or_else(|| model.id.clone())),
    );
    config.insert("tool_call".into(), json!(true));
    let mut limit = serde_json::Map::new();
    limit.insert(
        "context".into(),
        json!(model
            .limits
            .max_context_window_tokens
            .unwrap_or(DEFAULT_CONTEXT_WINDOW)),
    );
    limit.insert(
        "output".into(),
        json!(model.limits.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT)),
    );
    if let Some(input) = model.limits.max_prompt_tokens {
        limit.insert("input".into(), json!(input));
    }
    config.insert("limit".into(), Value::Object(limit));

    if model.chat.reasoning.is_some() {
        config.insert("reasoning".into(), json!(true));
        if let Some(effort) = model
            .chat
            .reasoning
            .as_ref()
            .and_then(|r| r.effort.as_ref())
        {
            let supported = effort.supported.clone().unwrap_or_default();
            if !supported.is_empty() {
                let mut variants = serde_json::Map::new();
                for level in &supported {
                    variants.insert(level.clone(), json!({ "reasoningEffort": level }));
                }
                for level in ["low", "medium", "high", "max"] {
                    if !supported.iter().any(|s| s == level) {
                        let entry = variants
                            .entry(level.to_string())
                            .or_insert_with(|| json!({}));
                        if let Some(map) = entry.as_object_mut() {
                            map.insert("disabled".into(), json!(true));
                        }
                    }
                }
                config.insert("variants".into(), Value::Object(variants));
            }
        }
    }

    if model.chat.modalities.input.iter().any(|m| m == "image") {
        config.insert("attachment".into(), json!(true));
    }
    let input = &model.chat.modalities.input;
    let output = &model.chat.modalities.output;
    if !input.is_empty() || !output.is_empty() {
        config.insert(
            "modalities".into(),
            json!({ "input": input, "output": output }),
        );
    }
    if let Some(created) = &model.created_at {
        if created.len() >= 10 {
            config.insert("release_date".into(), json!(&created[..10]));
        }
    }

    if let Some(rates) = default_rates(model) {
        if let (Some(input_rate), Some(output_rate)) = (
            rates.input_tokens.as_deref(),
            rates.output_tokens.as_deref(),
        ) {
            let mut cost = serde_json::Map::new();
            if let Some(v) = rate_f64(input_rate) {
                cost.insert("input".into(), json!(v));
            }
            if let Some(v) = rate_f64(output_rate) {
                cost.insert("output".into(), json!(v));
            }
            if let Some(v) = rates.input_cache_read_tokens.as_deref().and_then(rate_f64) {
                cost.insert("cache_read".into(), json!(v));
            }
            if let Some(v) = rates.input_cache_write_tokens.as_deref().and_then(rate_f64) {
                cost.insert("cache_write".into(), json!(v));
            }
            config.insert("cost".into(), Value::Object(cost));
        }
    }

    Value::Object(config)
}

pub fn unconfigure_opencode() -> Result<Option<String>> {
    let path = opencode_path();
    if unconfigure_json_provider(&path, &["provider"]) {
        return Ok(Some(format!("removed Floway from {}", path.display())));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Zed

pub fn apply_zed(client: &Client, models: &ModelList) -> Result<String> {
    let path = zed_path();
    let mut doc = json_doc::load_or_new(&path, "Zed settings")?;
    let root = json_doc::ensure_object(&mut doc, "root")?;
    let language_models = json_doc::ensure_object_in(root, "language_models")?;
    let openai_compatible = json_doc::ensure_object_in(language_models, "openai_compatible")?;

    let mut provider = serde_json::Map::new();
    provider.insert("api_url".into(), json!(format!("{}/v1", client.endpoint())));
    provider.insert(
        "available_models".into(),
        Value::Array(
            chat_models(models)
                .iter()
                .map(|m| zed_model_config(m))
                .collect(),
        ),
    );
    openai_compatible.insert(UPSTREAM.into(), Value::Object(provider));

    json_doc::save(&path, &doc, 0o600)?;
    Ok(format!("wrote {}", path.display()))
}

fn zed_model_config(model: &Model) -> Value {
    let mut config = serde_json::Map::new();
    config.insert("name".into(), json!(model.id));
    config.insert(
        "display_name".into(),
        json!(model
            .display_name
            .clone()
            .unwrap_or_else(|| model.id.clone())),
    );
    config.insert(
        "max_tokens".into(),
        json!(model
            .limits
            .max_context_window_tokens
            .unwrap_or(DEFAULT_CONTEXT_WINDOW)),
    );
    let image_capable = model.chat.modalities.input.iter().any(|m| m == "image");
    config.insert(
        "capabilities".into(),
        json!({
            "tools": true,
            "images": image_capable,
            "parallel_tool_calls": true,
            "prompt_cache_key": true,
            "chat_completions": false,
            "interleaved_reasoning": true,
        }),
    );
    Value::Object(config)
}

pub fn unconfigure_zed() -> Result<Option<String>> {
    let path = zed_path();
    if unconfigure_json_provider(&path, &["language_models", "openai_compatible"]) {
        return Ok(Some(format!("removed Floway from {}", path.display())));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// VSCode

pub fn apply_vscode(client: &Client, models: &ModelList) -> Result<String> {
    let path = vscode_path();
    let mut doc = json_doc::load_or_new(&path, "VSCode chat language models")?;
    if !doc.is_array() {
        doc = Value::Array(Vec::new());
    }
    let groups = doc.as_array_mut().unwrap();

    // Replace any prior Floway group; leave every unrelated group untouched.
    groups.retain(|group| {
        !(group.get("name").and_then(Value::as_str) == Some(UPSTREAM)
            && group.get("vendor").and_then(Value::as_str) == Some("customendpoint"))
    });

    let mut models_out = Vec::new();
    for model in chat_models(models) {
        models_out.push(vscode_model_config(
            model,
            &format!("{}/v1", client.endpoint()),
        ));
    }
    groups.push(json!({
        "name": UPSTREAM,
        "vendor": "customendpoint",
        "apiKey": client.api_key(),
        "apiType": "responses",
        "models": models_out,
    }));

    json_doc::save(&path, &doc, 0o600)?;
    Ok(format!("wrote {}", path.display()))
}

fn vscode_model_config(model: &Model, api_url: &str) -> Value {
    // Port of the converter's max-input/max-output inference table.
    let defaults = (
        DEFAULT_CONTEXT_WINDOW - DEFAULT_MAX_OUTPUT,
        DEFAULT_MAX_OUTPUT,
    );
    let ctx = model.limits.max_context_window_tokens;
    let input = model.limits.max_prompt_tokens;
    let output = model.limits.max_output_tokens;

    let (max_input, max_output) = match (ctx, input, output) {
        (None, None, None) => defaults,
        (None, None, Some(out)) => (DEFAULT_CONTEXT_WINDOW.saturating_sub(out).max(out), out),
        (None, Some(inp), None) => (inp, inp.max(DEFAULT_MAX_OUTPUT)),
        (None, Some(inp), Some(out)) => (inp.max(out), out),
        (Some(_ctx), Some(inp), Some(out)) => (inp, out),
        (Some(ctx), None, None) => {
            let out = (ctx / 2).min(DEFAULT_MAX_OUTPUT);
            (ctx - out, out)
        }
        (Some(ctx), None, Some(out)) => (ctx.saturating_sub(out), out),
        (Some(ctx), Some(inp), None) => (inp, ctx.saturating_sub(inp)),
    };

    let mut config = serde_json::Map::new();
    config.insert("id".into(), json!(model.id));
    config.insert(
        "name".into(),
        json!(model
            .display_name
            .clone()
            .unwrap_or_else(|| model.id.clone())),
    );
    config.insert(
        "url".into(),
        json!(format!("{}/responses", api_url.trim_end_matches('/'))),
    );
    config.insert("toolCalling".into(), json!(true));
    config.insert(
        "vision".into(),
        json!(model.chat.modalities.input.iter().any(|m| m == "image")),
    );
    config.insert("thinking".into(), json!(true));
    config.insert("maxInputTokens".into(), json!(max_input));
    config.insert("maxOutputTokens".into(), json!(max_output));
    config.insert("zeroDataRetentionEnabled".into(), json!(true));
    if let Some(reasoning) = &model.chat.reasoning {
        config.insert("reasoningEffortFormat".into(), json!("responses"));
        if let Some(supported) = reasoning.effort.as_ref().and_then(|e| e.supported.as_ref()) {
            if !supported.is_empty() {
                config.insert("supportsReasoningEffort".into(), json!(supported));
            }
        }
    }
    Value::Object(config)
}

pub fn unconfigure_vscode() -> Result<Option<String>> {
    let path = vscode_path();
    if !path.exists() {
        return Ok(None);
    }
    let doc = json_doc::load_or_new(&path, "VSCode chat language models")?;
    if !doc.is_array() {
        return Ok(None);
    }
    let mut doc = doc;
    let groups = doc.as_array_mut().unwrap();
    let before = groups.len();
    groups.retain(|group| {
        !(group.get("name").and_then(Value::as_str) == Some(UPSTREAM)
            && group.get("vendor").and_then(Value::as_str) == Some("customendpoint"))
    });
    if groups.len() == before {
        return Ok(None);
    }
    json_doc::save(&path, &doc, 0o600)?;
    Ok(Some(format!("removed Floway from {}", path.display())))
}

// ---------------------------------------------------------------------------
// DeepSeek Harness

pub fn apply_dsh(client: &Client, models: &ModelList) -> Result<String> {
    let (settings_path, credentials_path) = dsh_paths();
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut provider = serde_json::Map::new();
    provider.insert("displayName".into(), json!("Floway"));
    provider.insert("apiKeyEnv".into(), json!("FLOWAY_API_KEY"));
    provider.insert("api".into(), json!("openai-responses"));
    provider.insert(
        "baseURL".into(),
        json!(format!("{}/v1", client.endpoint().trim_end_matches('/'))),
    );
    provider.insert(
        "models".into(),
        Value::Array(
            chat_models(models)
                .iter()
                .map(|m| dsh_model_config(m))
                .collect(),
        ),
    );

    let mut doc = crate::yaml_doc::load_or_new(&settings_path, "DeepSeek Harness settings")?;
    let root = json_doc::ensure_object(&mut doc, "root")?;
    let pi_ai = json_doc::ensure_object_in(root, "llm-pi-ai")?;
    let providers = json_doc::ensure_object_in(pi_ai, "providers")?;
    providers.insert("floway".into(), Value::Object(provider));

    let settings_yaml = crate::yaml_doc::to_yaml(&doc)?;
    crate::write_private_file(&settings_path, &settings_yaml)?;

    let mut creds_doc =
        crate::yaml_doc::load_or_new(&credentials_path, "DeepSeek Harness credentials")?;
    let creds_root = json_doc::ensure_object(&mut creds_doc, "root")?;
    creds_root.insert("FLOWAY_API_KEY".into(), json!(client.api_key()));
    let creds_yaml = crate::yaml_doc::to_yaml(&creds_doc)?;
    crate::write_private_file(&credentials_path, &creds_yaml)?;

    Ok(format!(
        "wrote {} and the key in {}",
        settings_path.display(),
        credentials_path.display()
    ))
}

fn dsh_model_config(model: &Model) -> Value {
    let mut config = serde_json::Map::new();
    config.insert("id".into(), json!(model.id));
    config.insert(
        "name".into(),
        json!(model
            .display_name
            .clone()
            .unwrap_or_else(|| model.id.clone())),
    );
    let modalities = &model.chat.modalities.input;
    if modalities.iter().any(|m| m != "text") {
        config.insert("input".into(), json!(modalities));
    }
    if let Some(ctx) = model.limits.max_context_window_tokens {
        config.insert("contextWindow".into(), json!(ctx));
    }
    if let Some(out) = model.limits.max_output_tokens {
        config.insert("maxTokens".into(), json!(out));
    }
    if let Some(reasoning) = &model.chat.reasoning {
        if let Some(supported) = reasoning.effort.as_ref().and_then(|e| e.supported.as_ref()) {
            if !supported.is_empty() {
                let mut efforts = serde_json::Map::new();
                efforts.insert("off".into(), Value::Null);
                for level in supported {
                    efforts.insert(level.clone(), json!(level));
                }
                config.insert("reasoningEfforts".into(), Value::Object(efforts));
            }
        }
    }
    Value::Object(config)
}

pub fn unconfigure_dsh() -> Result<Option<String>> {
    let (settings_path, credentials_path) = dsh_paths();
    let mut touched = Vec::new();

    if settings_path.exists() {
        let text = std::fs::read_to_string(&settings_path)?;
        if let Ok(Value::Object(mut doc)) = crate::yaml_doc::from_yaml(&text) {
            if remove_provider_key(&mut doc, &["llm-pi-ai", "providers"], "floway") {
                if doc.is_empty() {
                    std::fs::remove_file(&settings_path)?;
                } else {
                    let body = crate::yaml_doc::to_yaml(&Value::Object(doc))?;
                    crate::write_private_file(&settings_path, &body)?;
                }
                touched.push(settings_path.display().to_string());
            }
        }
    }

    if credentials_path.exists() {
        let text = std::fs::read_to_string(&credentials_path)?;
        if let Ok(Value::Object(mut doc)) = crate::yaml_doc::from_yaml(&text) {
            if doc.remove("FLOWAY_API_KEY").is_some() {
                if doc.is_empty() {
                    std::fs::remove_file(&credentials_path)?;
                } else {
                    let body = crate::yaml_doc::to_yaml(&Value::Object(doc))?;
                    crate::write_private_file(&credentials_path, &body)?;
                }
                touched.push(credentials_path.display().to_string());
            }
        }
    }

    if touched.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("removed Floway from {}", touched.join(", "))))
}

// ---------------------------------------------------------------------------

/// Remove the `Floway` key at `path[..]/key`, pruning now-empty parents and
/// the `$schema` helper key opencode owns. Returns whether anything changed.
fn remove_provider(doc: &mut serde_json::Map<String, Value>, path: &[&str]) -> bool {
    remove_provider_key(doc, path, UPSTREAM)
}

fn remove_provider_key(doc: &mut serde_json::Map<String, Value>, path: &[&str], key: &str) -> bool {
    let mut changed = false;
    if path.is_empty() {
        if doc.remove(key).is_some() {
            changed = true;
        }
        // `$schema` was written by floway; drop it when the file is otherwise
        // a fresh document.
        if doc.len() == 1 && doc.contains_key("$schema") {
            doc.remove("$schema");
        }
        return changed;
    }
    if let Some(child) = doc.get_mut(path[0]) {
        if let Some(map) = child.as_object_mut() {
            changed = remove_provider_key(map, &path[1..], key);
            if map.is_empty() {
                doc.remove(path[0]);
            }
        }
    }
    changed
}

fn unconfigure_json_provider(path: &std::path::Path, parent_path: &[&str]) -> bool {
    if !path.exists() {
        return false;
    }
    let doc = match json_doc::load_or_new(path, "config") {
        Ok(doc) => doc,
        Err(_) => return false,
    };
    if !doc.is_object() {
        return false;
    }
    let mut doc = doc;
    let removed = {
        let root = doc.as_object_mut().unwrap();
        remove_provider(root, parent_path)
    };
    if !removed {
        return false;
    }
    json_doc::save(path, &doc, 0o600).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn omp_model_config_cost_includes_all_required_fields() {
        let model: Model = serde_json::from_value(json!({
            "id": "test-model",
            "pricing": {
                "entries": [
                    {
                        "rates": {
                            "input_tokens": "0.00000174",
                            "output_tokens": "0.00000348",
                            "input_cache_read_tokens": "0.0000000145"
                        }
                    }
                ]
            }
        }))
        .unwrap();

        let cfg = omp_model_config(&model);
        let cost = cfg
            .get("cost")
            .expect("cost should be present")
            .as_object()
            .unwrap();
        assert_eq!(cost.get("input").and_then(Value::as_f64), Some(1.74));
        assert_eq!(cost.get("output").and_then(Value::as_f64), Some(3.48));
        assert_eq!(cost.get("cacheRead").and_then(Value::as_f64), Some(0.0145));
        assert_eq!(cost.get("cacheWrite").and_then(Value::as_f64), Some(0.0));
    }

    #[test]
    fn dsh_model_config_reasoning_and_modalities() {
        let model: Model = serde_json::from_value(json!({
            "id": "gpt-5.6",
            "display_name": "GPT-5.6",
            "kind": "chat",
            "limits": {
                "max_context_window_tokens": 400000,
                "max_output_tokens": 100000
            },
            "chat": {
                "modalities": {"input": ["text", "image"], "output": ["text"]},
                "reasoning": {
                    "effort": {
                        "supported": ["low", "medium", "high"],
                        "default": "medium"
                    }
                }
            }
        }))
        .unwrap();

        let cfg = dsh_model_config(&model);
        assert_eq!(cfg.get("id").and_then(Value::as_str), Some("gpt-5.6"));
        assert_eq!(cfg.get("name").and_then(Value::as_str), Some("GPT-5.6"));
        assert_eq!(
            cfg.get("contextWindow").and_then(Value::as_u64),
            Some(400000)
        );
        assert_eq!(cfg.get("maxTokens").and_then(Value::as_u64), Some(100000));
        assert_eq!(
            cfg.get("input").and_then(Value::as_array).map(|a| a.len()),
            Some(2)
        );

        let efforts = cfg
            .get("reasoningEfforts")
            .expect("reasoningEfforts should be present")
            .as_object()
            .unwrap();
        assert!(efforts.contains_key("off"));
        assert_eq!(efforts.get("off"), Some(&Value::Null));
        assert_eq!(efforts.get("low").and_then(Value::as_str), Some("low"));
        assert_eq!(efforts.get("medium").and_then(Value::as_str), Some("medium"));
        assert_eq!(efforts.get("high").and_then(Value::as_str), Some("high"));
        assert!(cfg.get("cost").is_none(), "dsh model config must not have cost");
    }

    #[test]
    fn apply_then_unconfigure_dsh_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-dsh-e2e-{}", std::process::id()));
        let dsh_dir = dir.join(".dsh");
        std::fs::create_dir_all(&dsh_dir).unwrap();
        std::env::set_var("DSH_CONFIG_DIR", &dsh_dir);

        let client =
            crate::gateway::Client::new("http://127.0.0.1:18099".into(), "test-key".into())
                .unwrap();
        let model: Model = serde_json::from_value(json!({
            "id": "gpt-5.6",
            "display_name": "GPT-5.6",
            "kind": "chat",
            "limits": {
                "max_context_window_tokens": 400000,
                "max_output_tokens": 100000
            },
            "chat": {
                "modalities": {"input": ["text", "image"], "output": ["text"]},
                "reasoning": {
                    "effort": {
                        "supported": ["low", "medium", "high"],
                        "default": "medium"
                    }
                }
            }
        }))
        .unwrap();
        let models = crate::gateway::ModelList {
            data: vec![model],
        };

        apply_dsh(&client, &models).unwrap();

        let (settings_path, creds_path) = dsh_paths();
        assert!(settings_path.exists(), "settings.yaml must exist");
        assert!(creds_path.exists(), ".credentials.yaml must exist");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&creds_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credentials file must be 0600");
        }

        // Verify content in settings.yaml
        let settings_raw = std::fs::read_to_string(&settings_path).unwrap();
        assert!(settings_raw.contains("floway:"));
        assert!(settings_raw.contains("baseURL: \"http://127.0.0.1:18099/v1\""));
        assert!(settings_raw.contains("apiKeyEnv: FLOWAY_API_KEY"));
        assert!(settings_raw.contains("api: openai-responses"));
        assert!(settings_raw.contains("\"off\": null"));

        // Verify credentials
        let creds_raw = std::fs::read_to_string(&creds_path).unwrap();
        assert!(creds_raw.contains("FLOWAY_API_KEY: test-key"));

        // Inject foreign keys in settings.yaml and .credentials.yaml
        let mut settings_val = crate::yaml_doc::from_yaml(&settings_raw).unwrap();
        settings_val
            .as_object_mut()
            .unwrap()
            .insert("custom_key".into(), json!("keep-me"));
        let modified_settings = crate::yaml_doc::to_yaml(&settings_val).unwrap();
        std::fs::write(&settings_path, modified_settings).unwrap();

        let mut creds_val = crate::yaml_doc::from_yaml(&creds_raw).unwrap();
        creds_val
            .as_object_mut()
            .unwrap()
            .insert("OTHER_KEY".into(), json!("keep-secret"));
        let modified_creds = crate::yaml_doc::to_yaml(&creds_val).unwrap();
        std::fs::write(&creds_path, modified_creds).unwrap();

        // Unconfigure
        let unconf = unconfigure_dsh().unwrap();
        assert!(unconf.is_some());

        // Foreign keys must survive
        assert!(settings_path.exists());
        assert!(creds_path.exists());

        let after_settings: Value =
            crate::yaml_doc::from_yaml(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            after_settings.get("custom_key").and_then(Value::as_str),
            Some("keep-me")
        );
        assert!(after_settings.get("llm-pi-ai").is_none());

        let after_creds: Value =
            crate::yaml_doc::from_yaml(&std::fs::read_to_string(&creds_path).unwrap()).unwrap();
        assert_eq!(
            after_creds.get("OTHER_KEY").and_then(Value::as_str),
            Some("keep-secret")
        );
        assert!(after_creds.get("FLOWAY_API_KEY").is_none());

        // Second round trip without foreign keys: should delete files completely
        let _ = std::fs::remove_file(&settings_path);
        let _ = std::fs::remove_file(&creds_path);
        apply_dsh(&client, &models).unwrap();
        assert!(settings_path.exists());
        assert!(creds_path.exists());

        let unconf2 = unconfigure_dsh().unwrap();
        assert!(unconf2.is_some());
        assert!(
            !settings_path.exists(),
            "settings.yaml should be removed completely when only floway was configured"
        );
        assert!(
            !creds_path.exists(),
            ".credentials.yaml should be removed completely when only FLOWAY_API_KEY was configured"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    fn test_model() -> Model {
        serde_json::from_value(json!({
            "id": "gpt-5.6",
            "display_name": "GPT-5.6",
            "kind": "chat",
            "limits": {
                "max_context_window_tokens": 400000,
                "max_output_tokens": 100000
            },
            "chat": {
                "modalities": {"input": ["text", "image"], "output": ["text"]},
                "reasoning": {
                    "effort": {
                        "supported": ["low", "medium", "high"],
                        "default": "medium"
                    }
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn apply_then_unconfigure_omp_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-omp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PI_CODING_AGENT_DIR", &dir);
        std::env::remove_var("PI_CONFIG_DIR");

        let client = Client::new("http://gw.example".into(), "test-key".into()).unwrap();
        let models = ModelList {
            data: vec![test_model()],
        };

        apply_omp(&client, &models).unwrap();

        let (models_path, env_path) = omp_paths();
        assert!(models_path.exists());
        assert!(env_path.exists());

        // Inject foreign settings into both models.yml and .env
        let models_val: Value =
            crate::yaml_doc::from_yaml(&std::fs::read_to_string(&models_path).unwrap()).unwrap();
        let mut models_val = models_val;
        models_val
            .get_mut("providers")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("OtherProvider".into(), json!({ "baseUrl": "http://other" }));
        let modified_models = crate::yaml_doc::to_yaml(&models_val).unwrap();
        std::fs::write(&models_path, modified_models).unwrap();

        let mut env_content = std::fs::read_to_string(&env_path).unwrap();
        env_content.push_str("OTHER_ENV=val\n");
        std::fs::write(&env_path, env_content).unwrap();

        // Re-apply (update) must preserve foreign provider and env
        apply_omp(&client, &models).unwrap();
        let after_update_models = std::fs::read_to_string(&models_path).unwrap();
        assert!(after_update_models.contains("OtherProvider"));
        assert!(after_update_models.contains("Floway"));
        let after_update_env = std::fs::read_to_string(&env_path).unwrap();
        assert!(after_update_env.contains("OTHER_ENV=val"));
        assert!(after_update_env.contains("FLOWAY_API_KEY="));

        // Unconfigure
        let unconf = unconfigure_omp().unwrap();
        assert!(unconf.is_some());

        // Foreign provider survives, Floway removed
        assert!(models_path.exists());
        assert!(env_path.exists());
        let after_unconf_models = std::fs::read_to_string(&models_path).unwrap();
        assert!(after_unconf_models.contains("OtherProvider"));
        assert!(!after_unconf_models.contains("Floway"));
        let after_unconf_env = std::fs::read_to_string(&env_path).unwrap();
        assert!(after_unconf_env.contains("OTHER_ENV=val"));
        assert!(!after_unconf_env.contains("FLOWAY_API_KEY="));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_then_unconfigure_pi_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-pi-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PI_CODING_AGENT_DIR", &dir);

        let client = Client::new("http://gw.example".into(), "test-key-pi".into()).unwrap();
        let models = ModelList {
            data: vec![test_model()],
        };

        apply_pi(&client, &models).unwrap();

        let models_path = pi_path();
        assert_eq!(models_path, dir.join("models.json"));
        assert!(models_path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&models_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "models.json must be 0600");
        }

        // Verify content in models.json
        let content = std::fs::read_to_string(&models_path).unwrap();
        let val: Value = serde_json::from_str(&content).unwrap();
        let floway = val
            .get("providers")
            .and_then(|p| p.get("Floway"))
            .expect("Floway provider must exist");
        assert_eq!(
            floway.get("baseUrl").and_then(Value::as_str),
            Some("http://gw.example/v1")
        );
        assert_eq!(
            floway.get("apiKey").and_then(Value::as_str),
            Some("test-key-pi")
        );
        assert_eq!(
            floway.get("api").and_then(Value::as_str),
            Some("openai-responses")
        );
        let models_arr = floway.get("models").and_then(Value::as_array).unwrap();
        assert_eq!(models_arr.len(), 1);
        assert_eq!(models_arr[0].get("id").and_then(Value::as_str), Some("gpt-5.6"));

        // Inject foreign settings (foreign provider and foreign top-level key)
        let mut doc = val;
        doc.get_mut("providers")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("OtherProvider".into(), json!({ "baseUrl": "http://other" }));
        doc.as_object_mut()
            .unwrap()
            .insert("customField".into(), json!("customValue"));
        std::fs::write(&models_path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        // Re-apply (update) must preserve foreign provider and top-level field
        apply_pi(&client, &models).unwrap();
        let after_update_raw = std::fs::read_to_string(&models_path).unwrap();
        let after_update: Value = serde_json::from_str(&after_update_raw).unwrap();
        assert!(after_update.get("providers").unwrap().get("OtherProvider").is_some());
        assert!(after_update.get("providers").unwrap().get("Floway").is_some());
        assert_eq!(
            after_update.get("customField").and_then(Value::as_str),
            Some("customValue")
        );

        // Unconfigure
        let unconf = unconfigure_pi().unwrap();
        assert!(unconf.is_some());

        // Foreign provider survives, Floway removed
        assert!(models_path.exists());
        let after_unconf_raw = std::fs::read_to_string(&models_path).unwrap();
        let after_unconf: Value = serde_json::from_str(&after_unconf_raw).unwrap();
        assert!(after_unconf.get("providers").unwrap().get("OtherProvider").is_some());
        assert!(after_unconf.get("providers").unwrap().get("Floway").is_none());
        assert_eq!(
            after_unconf.get("customField").and_then(Value::as_str),
            Some("customValue")
        );

        // Round-trip without foreign keys: should delete models.json completely
        let _ = std::fs::remove_file(&models_path);
        apply_pi(&client, &models).unwrap();
        assert!(models_path.exists());

        let unconf2 = unconfigure_pi().unwrap();
        assert!(unconf2.is_some());
        assert!(
            !models_path.exists(),
            "models.json should be removed completely when only floway was configured"
        );

        // Pi's config dir comes from PI_CODING_AGENT_DIR only; PI_CONFIG_DIR is
        // oh-my-pi's variable and must not steer the Pi writer.
        std::env::remove_var("PI_CODING_AGENT_DIR");
        let ignored = dir.join("config_root");
        std::env::set_var("PI_CONFIG_DIR", &ignored);
        let home = crate::fs_util::home_dir();
        assert_eq!(
            pi_path(),
            home.join(".pi").join("agent").join("models.json"),
            "PI_CONFIG_DIR must not affect Pi's models.json location"
        );
        std::env::remove_var("PI_CONFIG_DIR");

        // Pi expands a leading `~` itself, so the writer must expand it too
        // rather than creating a literal `~` directory.
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &dir);
        std::env::set_var("PI_CODING_AGENT_DIR", "~/agent");
        assert_eq!(pi_path(), dir.join("agent").join("models.json"));
        match original_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
        std::env::set_var("PI_CODING_AGENT_DIR", &dir);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A blank `display_name` must not be written as `name`, and a blank `id`
    /// must be skipped: Pi rejects the whole file on either, which would take
    /// every unrelated provider down with it.
    #[test]
    fn pi_model_config_survives_blank_display_names() {
        let mut model = test_model();
        model.display_name = Some(String::new());
        let config = pi_model_config(&model).expect("model with an id is kept");
        assert_eq!(
            config.get("name").and_then(Value::as_str),
            Some("gpt-5.6"),
            "a blank display_name falls back to the model id"
        );

        let mut nameless = test_model();
        nameless.display_name = None;
        assert_eq!(
            pi_model_config(&nameless)
                .unwrap()
                .get("name")
                .and_then(Value::as_str),
            Some("gpt-5.6")
        );

        let mut idless = test_model();
        idless.id = String::new();
        assert!(
            pi_model_config(&idless).is_none(),
            "a model with no id cannot be represented in models.json"
        );
    }

    /// Pi resolves `apiKey` as a config value template, so the writer escapes
    /// it. This decodes the escaped form with Pi's documented rules and checks
    /// the original literal comes back.
    #[test]
    fn pi_config_value_round_trips_through_pi_resolution() {
        fn resolve(config: &str) -> String {
            // Pi's parseConfigValueTemplate: `$$` -> `$`, `$!` -> `!`,
            // `$VAR`/`${VAR}` -> env lookup, `!cmd` at the start -> command.
            let mut out = String::new();
            let mut chars = config.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '$' {
                    out.push(c);
                    continue;
                }
                match chars.peek() {
                    Some('$') => {
                        chars.next();
                        out.push('$');
                    }
                    Some('!') => {
                        chars.next();
                        out.push('!');
                    }
                    _ => out.push('$'),
                }
            }
            out
        }

        for literal in [
            "sk-fw-1234",
            "sk-a$b-c",
            "$leading",
            "$",
            "${CURLY}",
            "sk-$$already",
        ] {
            let written = pi_config_value(literal);
            assert!(
                !written.starts_with('!'),
                "{written:?} would be executed as a shell command by Pi"
            );
            assert_eq!(
                resolve(&written),
                literal,
                "{written:?} must resolve back to {literal:?}"
            );
        }
    }

    /// `unconfigure` keeps the `providers` key whenever anything else survives,
    /// because Pi rejects a models.json that lacks it — which would unload the
    /// user's own providers.
    #[test]
    fn unconfigure_pi_keeps_a_loadable_document() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-pi-unconf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PI_CODING_AGENT_DIR", &dir);

        let path = pi_path();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "providers": { "Floway": { "baseUrl": "http://gw.example/v1" } },
                "customField": "keep",
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(unconfigure_pi().unwrap().is_some());

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            after.get("customField").and_then(Value::as_str),
            Some("keep")
        );
        assert_eq!(
            after.get("providers"),
            Some(&Value::Object(serde_json::Map::new())),
            "Pi requires the providers key to exist"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Pi parses models.json as JSONC with an optional BOM, so a file Pi can
    /// load must not be rejected by the writer.
    #[test]
    fn apply_pi_accepts_the_jsonc_superset_pi_accepts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-pi-jsonc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PI_CODING_AGENT_DIR", &dir);

        let path = pi_path();
        std::fs::write(
            &path,
            "\u{feff}{\n  // my notes\n  \"providers\": {\n    \"Mine\": {\"baseUrl\": \"http://other\",},\n  },\n}\n",
        )
        .unwrap();

        let client = Client::new("http://gw.example".into(), "k".into()).unwrap();
        let models = ModelList {
            data: vec![test_model()],
        };
        apply_pi(&client, &models).unwrap();

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after.get("providers").unwrap().get("Mine").is_some());
        assert!(after.get("providers").unwrap().get("Floway").is_some());

        std::fs::remove_dir_all(&dir).ok();
    }
}
