//! Filesystem utilities: transactional file write with atomic rename and permission control.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

/// Atomically write `bytes` to `path` via a same-directory stage file and rename.
/// Preserves the file extension in `<stem>.floway-stage.<pid>.<ext>` so a stale stage
/// file is recognizable to the owning app.
/// On Unix, the stage file is created with the requested `mode` before writing,
/// avoiding permission windows.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create directory {}", parent.display()))?;
    }

    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stage_name = match path.extension() {
        Some(ext) if !ext.is_empty() => format!(
            "{}.floway-stage.{}.{}",
            file_stem,
            std::process::id(),
            ext.to_string_lossy()
        ),
        _ => format!("{}.floway-stage.{}", file_stem, std::process::id()),
    };
    let stage = path.with_file_name(stage_name);

    {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(mode);
        let mut file = options
            .open(&stage)
            .with_context(|| format!("could not create stage file {}", stage.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("could not write stage file {}", stage.display()))?;
        file.flush()
            .with_context(|| format!("could not flush stage file {}", stage.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("could not set permissions on {}", stage.display()))?;
        }
    }

    #[cfg(windows)]
    let _ = std::fs::remove_file(path);

    std::fs::rename(&stage, path)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}
