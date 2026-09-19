//! JSON document helpers shared by the config writers: load (or create),
//! transactional same-directory staging with a 0600 rename, and object
//! coercion for dotted parents.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Load a JSON document, or `{}` when the file does not exist. Rejects a
/// present-but-invalid document so a corrupt config is never clobbered.
pub fn load_or_new(path: &Path, label: &str) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(&raw).with_context(|| format!("{path:?} is not valid {label}"))
}

/// Load a JSON document written in the JSONC superset Pi accepts for
/// `models.json`: a leading UTF-8 BOM, `//` line comments, and trailing commas
/// before `}`/`]`. Pi's loader takes this superset, so a file Pi can read must
/// not be rejected here. Rejects a present-but-invalid document so a corrupt
/// config is never clobbered.
pub fn load_or_new_jsonc(path: &Path, label: &str) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let body = strip_jsonc(raw.strip_prefix('\u{feff}').unwrap_or(&raw));
    if body.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(&body).with_context(|| format!("{path:?} is not valid {label}"))
}

/// Strip `//` line comments and trailing commas, mirroring Pi's
/// `stripJsonComments`. Comment markers and commas inside string literals are
/// preserved; only ASCII bytes are dropped, so the result stays valid UTF-8.
pub fn strip_jsonc(input: &str) -> String {
    String::from_utf8(strip_trailing_commas(&strip_line_comments(input)))
        .expect("only ASCII bytes are removed from valid UTF-8")
}

fn strip_line_comments(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            // Drop the comment, keeping the newline so line structure survives.
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        out.push(b);
        i += 1;
    }
    out
}

fn strip_trailing_commas(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < input.len() {
        let b = input[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == b',' {
            let mut j = i + 1;
            while j < input.len() && input[j].is_ascii_whitespace() {
                j += 1;
            }
            if matches!(input.get(j), Some(b'}') | Some(b']')) {
                i += 1;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    out
}

/// Ensure `value` is a JSON object (replacing non-objects) and return its map.
pub fn ensure_object<'a>(
    value: &'a mut Value,
    label: &str,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    if !value.is_object() {
        *value = Value::Object(serde_json::Map::new());
    }
    let _ = label;
    Ok(value.as_object_mut().expect("just coerced to object"))
}

/// Ensure the key inside `map` holds an object (coercing) and return it.
pub fn ensure_object_in<'a>(
    map: &'a mut serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    let child = map
        .entry(key.to_string())
        .or_insert(Value::Object(serde_json::Map::new()));
    if !child.is_object() {
        *child = Value::Object(serde_json::Map::new());
    }
    Ok(child.as_object_mut().expect("just coerced to object"))
}

/// Atomically replace `path` with `doc`, staging in the same directory with
/// the requested mode.
pub fn save(path: &Path, doc: &Value, mode: u32) -> Result<()> {
    let mut body = serde_json::to_string_pretty(doc)?;
    body.push('\n');
    crate::fs_util::write_atomic(path, body.as_bytes(), mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parses(input: &str) -> Value {
        serde_json::from_str(&strip_jsonc(input)).expect("stripped text must be valid JSON")
    }

    /// `//` inside a string literal (base URLs, keys) must survive untouched,
    /// and `http://` must not be mistaken for a comment.
    #[test]
    fn strip_jsonc_preserves_comment_like_string_contents() {
        let doc = parses(
            r#"{
                // a real comment
                "baseUrl": "http://127.0.0.1:18099/v1", // trailing comment
                "note": "not // a comment",
                "escaped": "quote \" then // still text"
            }"#,
        );
        assert_eq!(
            doc.get("baseUrl").and_then(Value::as_str),
            Some("http://127.0.0.1:18099/v1")
        );
        assert_eq!(
            doc.get("note").and_then(Value::as_str),
            Some("not // a comment")
        );
        assert_eq!(
            doc.get("escaped").and_then(Value::as_str),
            Some("quote \" then // still text")
        );
    }

    #[test]
    fn strip_jsonc_removes_trailing_commas_at_every_depth() {
        let doc = parses(r#"{"a": [1, 2,], "b": {"c": 3,},}"#);
        assert_eq!(doc["a"].as_array().map(Vec::len), Some(2));
        assert_eq!(doc["b"]["c"].as_u64(), Some(3));
        // A comma inside a string is untouched.
        let doc = parses(r#"{"a": "x,}"}"#);
        assert_eq!(doc["a"].as_str(), Some("x,}"));
    }

    #[test]
    fn strip_jsonc_leaves_plain_json_byte_identical() {
        let input = "{\n  \"a\": [1, 2],\n  \"b\": \"c\"\n}\n";
        assert_eq!(strip_jsonc(input), input);
    }

    #[test]
    fn load_or_new_jsonc_accepts_bom_and_rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("floway-jsonc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("models.json");

        std::fs::write(&path, "\u{feff}{\"providers\": {}}\n").unwrap();
        assert_eq!(
            load_or_new_jsonc(&path, "test").unwrap()["providers"],
            Value::Object(serde_json::Map::new())
        );

        // A present-but-corrupt document is still rejected, BOM or not.
        std::fs::write(&path, "\u{feff}{oops").unwrap();
        assert!(load_or_new_jsonc(&path, "test").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
