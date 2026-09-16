//! Filesystem utilities: transactional file write with atomic rename and permission control.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

/// Atomically write `bytes` to `path` via a same-directory stage file and rename.
/// Preserves the file extension in `<stem>.floway-stage.<pid>.<ext>` so a stale stage
/// file is recognizable to the owning app.
/// On Unix, the stage file is created with the requested `mode` before writing,
/// avoiding permission windows.
/// Resolve the user's home directory.
/// Honors `HOME` first (used on Unix and in tests), then `USERPROFILE` (standard on Windows).
/// Defaults to `"."` if neither is set.
pub fn home_dir() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return std::path::PathBuf::from(home);
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            return std::path::PathBuf::from(profile);
        }
    }
    std::path::PathBuf::from(".")
}

/// Atomically write `bytes` to `path` via a same-directory stage file and rename.
/// Preserves the file extension in `<stem>.floway-stage.<pid>.<ext>` so a stale stage
/// file is recognizable to the owning app.
/// On Unix, the stage file is created with the requested `mode` before writing,
/// avoiding permission windows.
pub fn write_atomic(path: &Path, bytes: &[u8], #[allow(unused_variables)] mode: u32) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn home_dir_prefers_home_then_userprofile() {
        let _guard = ENV_LOCK.lock().unwrap();
        let orig_home = std::env::var("HOME").ok();
        let orig_profile = std::env::var("USERPROFILE").ok();

        std::env::set_var("HOME", "/custom/home");
        std::env::set_var("USERPROFILE", "C:\\Users\\Custom");
        assert_eq!(home_dir(), std::path::PathBuf::from("/custom/home"));

        std::env::remove_var("HOME");
        assert_eq!(home_dir(), std::path::PathBuf::from("C:\\Users\\Custom"));

        std::env::remove_var("USERPROFILE");
        assert_eq!(home_dir(), std::path::PathBuf::from("."));

        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match orig_profile {
            Some(p) => std::env::set_var("USERPROFILE", p),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
}

