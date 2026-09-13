//! toml_edit document save helper: rename a same-directory stage over the
//! target so a partial write can never be observed.

use anyhow::Result;
use std::path::Path;

pub fn save(path: &Path, doc: &toml_edit::DocumentMut) -> Result<()> {
    crate::fs_util::write_atomic(path, doc.to_string().as_bytes(), 0o644)
}
