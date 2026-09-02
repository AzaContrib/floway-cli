//! Harness agents: oh-my-pi, opencode, Zed, and VSCode. Rust ports of the
//! four Floway Python converters (`floway-to-{omp,opencode,zed,vscode}.py`)
//! plus native merge/unmerge writers replacing the bash `jq` merges.
//!
//! Each writer touches only the `Floway` provider subtree it owns and leaves
//! the rest of the document intact; unconfigure removes exactly that subtree.

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

pub fn omp_paths() -> (PathBuf, PathBuf) {
    let dir = match std::env::var("OMP_CONFIG_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".omp").join("agent")
        }
    };
    (dir.join("models.yml"), dir.join(".env"))
}

pub fn opencode_path() -> PathBuf {
    let dir = match std::env::var("OPENCODE_CONFIG_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".config").join("opencode")
        }
    };
    dir.join("opencode.json")
}

pub fn zed_path() -> PathBuf {
    let dir = match std::env::var("ZED_CONFIG_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".config").join("zed")
        }
    };
    dir.join("global_settings.json")
}

pub fn vscode_path() -> PathBuf {
    if let Ok(dir) = std::env::var("VSCODE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("chatLanguageModels.json");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(&home).join("Library/Application Support/Code/User");
    #[cfg(not(target_os = "macos"))]
    let base = PathBuf::from(&home).join(".config/Code/User");
    base.join("chatLanguageModels.json")
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
    std::fs::create_dir_all(models_path.parent().unwrap())?;

    let yaml = omp_settings_yaml(&provider)?;
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
    config.insert("reasoning".into(), json!(true));
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
            if let Some(v) = input_tokens.and_then(rate_f64) {
                cost.insert("input".into(), json!(v));
            }
            if let Some(v) = output_tokens.and_then(rate_f64) {
                cost.insert("output".into(), json!(v));
            }
            if let Some(v) = cache_read.and_then(rate_f64) {
                cost.insert("cacheRead".into(), json!(v));
            }
            if let Some(v) = cache_write.and_then(rate_f64) {
                cost.insert("cacheWrite".into(), json!(v));
            }
            config.insert("cost".into(), Value::Object(cost));
        }
    }
    config.insert("compat".into(), json!({ "supportsStore": true }));
    Value::Object(config)
}

/// Emit `providers: { Floway: {...} }` YAML matching PyYAML's safe_dump block
// style closely enough for oh-my-pi's parser.
fn omp_settings_yaml(provider: &serde_json::Map<String, Value>) -> Result<String> {
    let settings = json!({ "providers": { UPSTREAM: Value::Object(provider.clone()) } });
    crate::yaml_doc::to_yaml(&settings)
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

/// Remove the `Floway` key at `path[..]/key`, pruning now-empty parents and
/// the `$schema` helper key opencode owns. Returns whether anything changed.
fn remove_provider(doc: &mut serde_json::Map<String, Value>, path: &[&str]) -> bool {
    let mut changed = false;
    if path.is_empty() {
        if doc.remove(UPSTREAM).is_some() {
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
            changed = remove_provider(map, &path[1..]);
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
