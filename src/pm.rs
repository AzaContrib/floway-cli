//! Node.js / Bun package manager detection and command generation.
//!
//! Detects whether `pnpm`, `bun`, `yarn`, or `npm` is available or was used
//! to install an agent, and formats the appropriate global install/update commands.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Pnpm,
    Bun,
    Yarn,
    Npm,
}

impl PackageManager {
    pub fn name(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm",
            PackageManager::Bun => "bun",
            PackageManager::Yarn => "yarn",
            PackageManager::Npm => "npm",
        }
    }

    /// Global install/update command for a package (e.g. `@openai/codex@latest`).
    pub fn global_install_command(self, package: &str) -> String {
        match self {
            PackageManager::Pnpm => format!("pnpm add --global {package}"),
            PackageManager::Bun => format!("bun add --global {package}"),
            PackageManager::Yarn => format!("yarn global add {package}"),
            PackageManager::Npm => format!("npm install --global {package}"),
        }
    }

    /// Detect the best package manager.
    ///
    /// Precedence:
    /// 1. `FLOWAY_PACKAGE_MANAGER` or `FLOWAY_PM` environment override (`pnpm`, `bun`, `yarn`, `npm`).
    /// 2. If a specific binary name is provided (e.g. `dsh` or `codex`), and that binary exists in PATH,
    ///    inspect its filesystem path and symlink target to see which package manager installed it.
    /// 3. Ambient environment hints (`npm_config_user_agent`, `PNPM_HOME`, `BUN_INSTALL`).
    /// 4. PATH traversal: if one package manager appears in an earlier directory in `PATH`, prefer it;
    ///    if multiple appear in the same directory, prefer `pnpm` > `bun` > `yarn` > `npm`.
    /// 5. Fallback to `npm`.
    pub fn detect_for_binary(binary: Option<&str>) -> Self {
        if let Some(pm) = Self::from_env() {
            return pm;
        }

        if let Some(bin) = binary {
            if let Some(pm) = Self::from_installed_binary(bin) {
                return pm;
            }
        }

        if let Some(pm) = Self::from_ambient_env() {
            return pm;
        }

        Self::from_system_path().unwrap_or(PackageManager::Npm)
    }

    /// Parse from explicit environment variable `FLOWAY_PACKAGE_MANAGER` or `FLOWAY_PM`.
    pub fn from_env() -> Option<Self> {
        let val = std::env::var("FLOWAY_PACKAGE_MANAGER")
            .or_else(|_| std::env::var("FLOWAY_PM"))
            .ok()?;
        match val.trim().to_ascii_lowercase().as_str() {
            "pnpm" => Some(PackageManager::Pnpm),
            "bun" => Some(PackageManager::Bun),
            "yarn" => Some(PackageManager::Yarn),
            "npm" => Some(PackageManager::Npm),
            _ => None,
        }
    }

    /// If `binary` is found on PATH, examine its path and symlink target.
    pub fn from_installed_binary(binary: &str) -> Option<Self> {
        let bin_path = find_in_path(binary)?;
        let mut candidates = vec![bin_path.clone()];
        if let Ok(target) = std::fs::canonicalize(&bin_path) {
            candidates.push(target);
        }
        for path in candidates {
            let s = path.to_string_lossy().to_ascii_lowercase();
            if s.contains("pnpm") {
                return Some(PackageManager::Pnpm);
            }
            if s.contains(".bun") || s.contains("/bun/") || s.contains("\\bun\\") {
                return Some(PackageManager::Bun);
            }
            if s.contains("yarn") {
                return Some(PackageManager::Yarn);
            }
            if s.contains("npm") {
                return Some(PackageManager::Npm);
            }
        }
        None
    }

    /// Inspect ambient environment variables that package managers set.
    fn from_ambient_env() -> Option<Self> {
        if let Ok(ua) = std::env::var("npm_config_user_agent") {
            let lower = ua.to_ascii_lowercase();
            if lower.starts_with("pnpm/") || lower.contains("pnpm") {
                return Some(PackageManager::Pnpm);
            }
            if lower.starts_with("bun/") || lower.contains("bun") {
                return Some(PackageManager::Bun);
            }
            if lower.starts_with("yarn/") || lower.contains("yarn") {
                return Some(PackageManager::Yarn);
            }
            if lower.starts_with("npm/") || lower.contains("npm") {
                return Some(PackageManager::Npm);
            }
        }
        if std::env::var("PNPM_HOME").is_ok() {
            return Some(PackageManager::Pnpm);
        }
        if std::env::var("BUN_INSTALL").is_ok() {
            return Some(PackageManager::Bun);
        }
        None
    }

    /// Search PATH directories in order. If a directory contains package manager executables,
    /// pick the highest-priority one in that directory (`pnpm` > `bun` > `yarn` > `npm`).
    pub fn from_system_path() -> Option<Self> {
        let path_var = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_var) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            if is_in_dir(&dir, "pnpm") {
                return Some(PackageManager::Pnpm);
            }
            if is_in_dir(&dir, "bun") {
                return Some(PackageManager::Bun);
            }
            if is_in_dir(&dir, "yarn") {
                return Some(PackageManager::Yarn);
            }
            if is_in_dir(&dir, "npm") {
                return Some(PackageManager::Npm);
            }
        }
        None
    }
}

pub fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(binary);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            for ext in &[".exe", ".cmd", ".bat"] {
                let with_ext = dir.join(format!("{binary}{ext}"));
                if is_executable(&with_ext) {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

fn is_in_dir(dir: &Path, binary: &str) -> bool {
    let candidate = dir.join(binary);
    if is_executable(&candidate) {
        return true;
    }
    #[cfg(windows)]
    {
        for ext in &[".exe", ".cmd", ".bat"] {
            let with_ext = dir.join(format!("{binary}{ext}"));
            if is_executable(&with_ext) {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = path.metadata() {
        meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
    } else {
        false
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    if let Ok(meta) = path.metadata() {
        meta.is_file()
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn global_install_command_formats() {
        assert_eq!(
            PackageManager::Npm.global_install_command("@openai/codex@latest"),
            "npm install --global @openai/codex@latest"
        );
        assert_eq!(
            PackageManager::Pnpm.global_install_command("@openai/codex@latest"),
            "pnpm add --global @openai/codex@latest"
        );
        assert_eq!(
            PackageManager::Yarn.global_install_command("@openai/codex@latest"),
            "yarn global add @openai/codex@latest"
        );
        assert_eq!(
            PackageManager::Bun.global_install_command("@openai/codex@latest"),
            "bun add --global @openai/codex@latest"
        );
    }

    #[test]
    fn detects_from_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();

        std::env::set_var("FLOWAY_PACKAGE_MANAGER", "bun");
        assert_eq!(PackageManager::from_env(), Some(PackageManager::Bun));
        assert_eq!(
            PackageManager::detect_for_binary(None),
            PackageManager::Bun
        );

        std::env::set_var("FLOWAY_PACKAGE_MANAGER", "pnpm");
        assert_eq!(PackageManager::from_env(), Some(PackageManager::Pnpm));

        std::env::set_var("FLOWAY_PACKAGE_MANAGER", "yarn");
        assert_eq!(PackageManager::from_env(), Some(PackageManager::Yarn));

        std::env::set_var("FLOWAY_PACKAGE_MANAGER", "npm");
        assert_eq!(PackageManager::from_env(), Some(PackageManager::Npm));

        std::env::remove_var("FLOWAY_PACKAGE_MANAGER");

        std::env::set_var("FLOWAY_PM", "bun");
        assert_eq!(PackageManager::from_env(), Some(PackageManager::Bun));
        std::env::remove_var("FLOWAY_PM");
    }

    #[test]
    fn detects_from_installed_binary() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("floway-pm-test-{}", std::process::id()));
        let pnpm_bin_dir = dir.join("fake-pnpm-global-bin");
        std::fs::create_dir_all(&pnpm_bin_dir).unwrap();

        let fake_dsh = pnpm_bin_dir.join("dsh");
        std::fs::write(&fake_dsh, "#!/bin/sh\necho dsh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_dsh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let orig_path = std::env::var_os("PATH");
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                pnpm_bin_dir.display(),
                orig_path.as_deref().unwrap_or_default().to_string_lossy()
            ),
        );

        let detected = PackageManager::from_installed_binary("dsh");
        assert_eq!(detected, Some(PackageManager::Pnpm));

        if let Some(orig) = orig_path {
            std::env::set_var("PATH", orig);
        } else {
            std::env::remove_var("PATH");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
