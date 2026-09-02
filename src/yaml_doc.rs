//! Minimal YAML emit/parse for the oh-my-pi `models.yml` writer. The emitted
//! shape is exactly one nested map (`providers → Floway → …`), so a small
//! block-style emitter covers it; parsing uses serde_yaml for the unmerge path.

use anyhow::{Context, Result};
use serde_json::Value;

pub fn to_yaml(value: &Value) -> Result<String> {
    let mut out = String::new();
    emit(value, 0, &mut out)?;
    Ok(out)
}

fn emit(value: &Value, indent: usize, out: &mut String) -> Result<()> {
    let pad = "  ".repeat(indent);
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str(&format!("{pad}{{}}\n"));
                return Ok(());
            }
            for (key, child) in map.iter() {
                emit_entry(&pad, key, child, out)?;
            }
            Ok(())
        }
        scalar => {
            let inline = yaml_scalar(scalar)?;
            out.push_str(&format!("{pad}{inline}\n"));
            Ok(())
        }
    }
}
/// Emit one `key: value` (or nested) entry under a caller-provided prefix
/// (pad plus optional `- ` list marker).
fn emit_entry(prefix: &str, key: &str, value: &Value, out: &mut String) -> Result<()> {
    let key = yaml_key(key);
    match value {
        Value::Object(map) if !map.is_empty() => {
            out.push_str(&format!("{prefix}{key}:\n"));
            for (child_key, child) in map {
                emit_entry(&format!("{prefix}  "), child_key, child, out)?;
            }
        }
        Value::Array(items)
            if items
                .iter()
                .all(|i| i.is_string() || i.is_number() || i.is_boolean() || i.is_null()) =>
        {
            let rendered: Vec<String> = items.iter().map(yaml_scalar).collect::<Result<_>>()?;
            out.push_str(&format!("{prefix}{key}: [{}]\n", rendered.join(", ")));
        }
        Value::Array(items) if items.is_empty() => {
            out.push_str(&format!("{prefix}{key}: []\n"));
        }
        Value::Array(items) => {
            // Block list of maps: a `key:` header, then `- ` on each item's
            // first key with aligned continuation keys beneath it.
            out.push_str(&format!("{prefix}{key}:\n"));
            for item in items {
                match item {
                    Value::Object(entry) if !entry.is_empty() => {
                        for (position, (entry_key, entry_value)) in entry.iter().enumerate() {
                            let marker = if position == 0 { "- " } else { "  " };
                            emit_entry(
                                &format!("{prefix}  {marker}"),
                                entry_key,
                                entry_value,
                                out,
                            )?;
                        }
                    }
                    other => {
                        let inline = yaml_scalar(other)?;
                        out.push_str(&format!("{prefix}  - {inline}\n"));
                    }
                }
            }
        }
        scalar => {
            let inline = yaml_scalar(scalar)?;
            out.push_str(&format!("{prefix}{key}: {inline}\n"));
        }
    }
    Ok(())
}

/// Quote a key unless it is a plain YAML identifier.
fn yaml_key(key: &str) -> String {
    let plain = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if plain {
        key.to_string()
    } else {
        serde_json::to_string(key).unwrap_or_else(|_| key.to_string())
    }
}

fn yaml_scalar(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            // Plain when unambiguous; JSON-quoted (valid YAML) otherwise.
            let plain = !s.is_empty()
                && !s.contains(':')
                && !s.contains('#')
                && !s.starts_with([
                    '&', '*', '?', '|', '>', '!', '%', '@', '`', ' ', '\'', '"', '[', ']', '{',
                    '}', ',',
                ])
                && !s.ends_with(' ')
                && s.chars().all(|c| c != '\n' && c != '\t');
            if plain
                && !matches!(
                    s.as_str(),
                    "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
                )
                && s.parse::<f64>().is_err()
            {
                s.to_string()
            } else {
                serde_json::to_string(s)?
            }
        }
        Value::Array(_) | Value::Object(_) => {
            anyhow::bail!("nested inline collections are not emitted; restructure the value")
        }
    })
}

pub fn from_yaml(text: &str) -> Result<Value> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(text).with_context(|| "could not parse the YAML document")?;
    Ok(serde_json::to_value(value)?)
}
