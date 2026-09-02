//! JSON document helpers shared by the config writers: load (or create),
//! transactional same-directory staging with a 0600 rename, and object
//! coercion for dotted parents.

use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Write;
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
/// the requested mode. Preserves the previous file's owner/mode where the
/// platform allows; the stage rename keeps concurrent readers consistent.
pub fn save(path: &Path, doc: &Value, mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let mut body = serde_json::to_string_pretty(doc)?;
    body.push('\n');

    let stage = path.with_extension(format!(
        "{}.floway-stage.{}",
        path.extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(mode);
        let mut file = options.open(&stage)?;
        file.write_all(body.as_bytes())?;
        file.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
    }
    std::fs::rename(&stage, path)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}
