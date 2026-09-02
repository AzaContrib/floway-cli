//! toml_edit document save helper: rename a same-directory stage over the
//! target so a partial write can never be observed.

use anyhow::{Context, Result};
use std::path::Path;

pub fn save(path: &Path, doc: &toml_edit::DocumentMut) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let stage = path.with_extension(format!("toml.floway-stage.{}", std::process::id()));
    std::fs::write(&stage, doc.to_string())
        .with_context(|| format!("could not stage {}", path.display()))?;
    std::fs::rename(&stage, path)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}
