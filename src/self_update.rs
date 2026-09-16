//! Self-update mechanism for the floway binary.
//!
//! Downloads official static release binaries from GitHub Releases,
//! verifies their SHA-256 checksums, and atomically stages and replaces
//! the currently executing binary on Unix and Windows.

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;

use crate::ui;

// --- options ---------------------------------------------------------------

#[derive(clap::Args, Debug, Clone, Default)]
pub struct Options {
    /// Target version/tag to install (e.g. v0.2.0 or 0.2.0). Defaults to latest release.
    #[arg(long)]
    pub version: Option<String>,

    /// Check if a newer version is available without installing it.
    #[arg(long)]
    pub check: bool,

    /// Force reinstall even if already on the requested/latest version.
    #[arg(long, short)]
    pub force: bool,

    /// Override GitHub repository (defaults to FLOWAY_CLI_REPO or AzaContrib/floway-cli).
    #[arg(long)]
    pub repo: Option<String>,
}

// --- platform target -------------------------------------------------------

/// Resolve the Rust target triple corresponding to the pre-built release artifact.
pub fn current_target() -> Result<&'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Ok("x86_64-unknown-linux-musl")
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        Ok("aarch64-unknown-linux-musl")
    }

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Ok("x86_64-apple-darwin")
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok("aarch64-apple-darwin")
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok("x86_64-pc-windows-msvc")
    }

    #[cfg(not(any(
        all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
        all(target_os = "macos", any(target_arch = "x86_64", target_arch = "aarch64")),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        bail!("unsupported platform architecture for binary self-update")
    }
}

// --- cleanup helper --------------------------------------------------------

/// Clean up leftover .exe.old file on Windows from previous self-updates.
pub fn cleanup_old_binary() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let old_exe = exe.with_extension("exe.old");
        if old_exe.exists() {
            let _ = std::fs::remove_file(&old_exe);
        }
    }
}

// --- version resolution ----------------------------------------------------

pub fn normalize_version(v: &str) -> &str {
    v.trim().strip_prefix('v').unwrap_or(v.trim())
}

pub fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let clean = normalize_version(v);
    let mut parts = clean.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch_str = parts.next().unwrap_or("0");
    let patch = patch_str.split(&['-', '+'][..]).next()?.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    if let (Some(cur), Some(cand)) = (parse_semver(current), parse_semver(candidate)) {
        cand > cur
    } else {
        normalize_version(candidate) != normalize_version(current)
    }
}

/// Resolve the latest release tag from GitHub without requiring authentication.
pub fn resolve_latest_version(client: &reqwest::blocking::Client, repo: &str) -> Result<String> {
    // 1. Try redirect on https://github.com/{repo}/releases/latest (doesn't count against API rate limit)
    let latest_page_url = format!("https://github.com/{repo}/releases/latest");
    if let Ok(resp) = client.get(&latest_page_url).send() {
        let final_url = resp.url().as_str();
        if let Some(tag_part) = final_url.split("/tag/").nth(1) {
            let tag = tag_part.trim_matches('/').split('?').next().unwrap_or(tag_part);
            if !tag.is_empty() {
                return Ok(tag.to_string());
            }
        }
    }

    // 2. Fallback to GitHub REST API
    let api_url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = client
        .get(&api_url)
        .send()
        .context("could not reach GitHub releases API")?;

    if resp.status().is_success() {
        let val: serde_json::Value = resp.json().context("invalid JSON from GitHub releases API")?;
        if let Some(tag) = val.get("tag_name").and_then(|t| t.as_str()) {
            return Ok(tag.to_string());
        }
    }

    bail!("could not determine the latest release tag for {repo}");
}

// --- checksum parser -------------------------------------------------------

/// Parse a SHA-256 sidecar file (single hash or two-column format).
pub fn parse_sha256_sidecar(content: &str, filename: &str) -> Result<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let hash = parts[0].to_ascii_lowercase();
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            if parts.len() == 1 {
                return Ok(hash);
            }
            let file_part = parts[1].trim_start_matches('*');
            if file_part == filename || file_part.ends_with(filename) {
                return Ok(hash);
            }
        }
    }
    bail!("no matching 64-character SHA-256 hash found in sidecar for {filename}");
}

// --- archive extractor -----------------------------------------------------

/// Extract the floway executable from an in-memory .tar.gz archive.
pub fn extract_binary_from_archive(archive_bytes: &[u8]) -> Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(gz);

    for entry_result in archive.entries().context("invalid tar archive in release asset")? {
        let mut entry = entry_result.context("failed reading tar entry")?;
        let path = entry.path().context("invalid entry path in archive")?;
        if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
            if file_name == "floway" || file_name == "floway.exe" {
                let mut binary = Vec::new();
                entry
                    .read_to_end(&mut binary)
                    .context("failed reading binary contents from archive")?;
                return Ok(binary);
            }
        }
    }

    bail!("the release archive did not contain a 'floway' binary");
}

// --- binary replacement ----------------------------------------------------

/// Stage and atomically replace the target executable file.
pub fn replace_executable(target_exe: &Path, new_binary_bytes: &[u8]) -> Result<()> {
    let parent_dir = target_exe
        .parent()
        .context("cannot find parent directory of executable")?;

    let tmp_name = format!(
        ".{}-update-{}",
        target_exe
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("floway"),
        std::process::id()
    );
    let tmp_path = parent_dir.join(tmp_name);

    std::fs::write(&tmp_path, new_binary_bytes).with_context(|| {
        format!(
            "failed to write temporary file {}. Check directory write permissions for {}",
            tmp_path.display(),
            parent_dir.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        if let Err(e) = std::fs::set_permissions(&tmp_path, perms) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e).context("failed to set executable permissions on temporary binary");
        }

        if let Err(e) = std::fs::rename(&tmp_path, target_exe) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e).with_context(|| {
                format!(
                    "failed to replace {} with new binary. Check permissions or run with elevated privileges (e.g. sudo).",
                    target_exe.display()
                )
            });
        }
    }

    #[cfg(windows)]
    {
        let old_exe = target_exe.with_extension("exe.old");
        let _ = std::fs::remove_file(&old_exe);

        if let Err(e) = std::fs::rename(target_exe, &old_exe) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e).with_context(|| {
                format!(
                    "failed to rename current binary {} to {}. Check file permissions.",
                    target_exe.display(),
                    old_exe.display()
                )
            });
        }

        if let Err(e) = std::fs::rename(&tmp_path, target_exe) {
            let _ = std::fs::rename(&old_exe, target_exe);
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e).with_context(|| {
                format!(
                    "failed to move new binary into place at {}. Rolled back to original binary.",
                    target_exe.display()
                )
            });
        }

        let _ = std::fs::remove_file(&old_exe);
    }

    Ok(())
}

// --- orchestration ---------------------------------------------------------

pub fn run(opts: Options) -> Result<()> {
    cleanup_old_binary();

    let target = current_target()?;
    let repo = opts
        .repo
        .or_else(|| std::env::var("FLOWAY_CLI_REPO").ok())
        .unwrap_or_else(|| "AzaContrib/floway-cli".to_string());

    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("floway-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to initialize HTTP client")?;

    let current_version = env!("CARGO_PKG_VERSION");
    let (target_tag, is_latest_check) = match opts.version {
        Some(v) => {
            let tag = if v.starts_with('v') { v } else { format!("v{v}") };
            (tag, false)
        }
        None => {
            println!("Checking for updates …");
            let latest = resolve_latest_version(&client, &repo)?;
            (latest, true)
        }
    };

    let is_newer = is_newer_version(current_version, &target_tag);

    if opts.check {
        if is_newer {
            println!(
                "An update is available: {} -> {} (run `floway self-update` to install)",
                ui::dim(&format!("v{current_version}")),
                ui::green(&target_tag)
            );
        } else {
            println!(
                "floway is up to date (current: v{}, latest: {}).",
                current_version, target_tag
            );
        }
        return Ok(());
    }

    if !is_newer && !opts.force && is_latest_check {
        println!(
            "floway is already up to date ({}). Use `--force` to reinstall.",
            ui::green(&format!("v{current_version}"))
        );
        return Ok(());
    }

    let archive_name = format!("floway-{target}.tar.gz");
    let download_url = format!("https://github.com/{repo}/releases/download/{target_tag}/{archive_name}");
    let checksum_url = format!("{download_url}.sha256");

    println!(
        "Downloading floway {} for {} …",
        ui::bold(&target_tag),
        target
    );
    let archive_resp = client
        .get(&download_url)
        .send()
        .with_context(|| format!("failed to download release archive from {download_url}"))?;

    if !archive_resp.status().is_success() {
        bail!(
            "failed to download release from {download_url}: HTTP {}",
            archive_resp.status()
        );
    }

    let archive_bytes = archive_resp
        .bytes()
        .context("failed to read downloaded archive")?;

    // Checksum verification if sidecar exists
    if let Ok(resp) = client.get(&checksum_url).send() {
        if resp.status().is_success() {
            if let Ok(text) = resp.text() {
                if let Ok(expected_hash) = parse_sha256_sidecar(&text, &archive_name) {
                    use sha2::Digest;
                    let actual_hash = format!("{:x}", sha2::Sha256::digest(&archive_bytes));
                    if actual_hash != expected_hash {
                        bail!(
                            "checksum mismatch for {archive_name}:\n  expected: {expected_hash}\n    actual: {actual_hash}"
                        );
                    }
                    println!("{}", ui::green("Checksum verified."));
                }
            }
        }
    }

    println!("Extracting binary …");
    let binary_bytes = extract_binary_from_archive(&archive_bytes)?;

    let current_exe = std::env::current_exe().context("could not locate current executable")?;
    let target_exe = match std::fs::canonicalize(&current_exe) {
        Ok(p) => {
            #[cfg(windows)]
            {
                let s = p.to_string_lossy();
                if let Some(stripped) = s.strip_prefix(r"\\?\") {
                    std::path::PathBuf::from(stripped)
                } else {
                    p
                }
            }
            #[cfg(not(windows))]
            p
        }
        Err(_) => current_exe,
    };

    replace_executable(&target_exe, &binary_bytes)?;
    println!(
        "{}",
        ui::green(&format!(
            "Successfully updated floway to {} ({})!",
            target_tag,
            target_exe.display()
        ))
    );

    Ok(())
}

// --- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    #[test]
    fn parses_sha256_sidecar_two_column() {
        let sidecar = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  floway-x86_64-unknown-linux-musl.tar.gz\n";
        let hash = parse_sha256_sidecar(sidecar, "floway-x86_64-unknown-linux-musl.tar.gz").unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn parses_sha256_sidecar_single_hash() {
        let sidecar = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n";
        let hash = parse_sha256_sidecar(sidecar, "floway-x86_64-unknown-linux-musl.tar.gz").unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn parses_sha256_sidecar_asterisk_binary_mode() {
        let sidecar = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *floway-x86_64-pc-windows-msvc.tar.gz\n";
        let hash = parse_sha256_sidecar(sidecar, "floway-x86_64-pc-windows-msvc.tar.gz").unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn extracts_binary_from_tar_gz() {
        let mut tar_builder = tar::Builder::new(Vec::new());
        let content = b"fake-binary-bytes-for-test-9876";

        let mut header = tar::Header::new_gnu();
        header.set_path("floway-target/floway").unwrap();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder.append(&header, &content[..]).unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let extracted = extract_binary_from_archive(&gz_bytes).unwrap();
        assert_eq!(extracted, content);
    }

    #[test]
    fn version_comparison_logic() {
        assert_eq!(normalize_version("v0.1.0"), "0.1.0");
        assert_eq!(normalize_version("0.1.0"), "0.1.0");
        assert!(is_newer_version("0.1.0", "v0.2.0"));
        assert!(is_newer_version("0.1.0", "0.1.1"));
        assert!(is_newer_version("0.1.0", "1.0.0"));
        assert!(!is_newer_version("0.2.0", "v0.1.0"));
        assert!(!is_newer_version("0.1.0", "v0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
    }

    #[test]
    fn target_resolves_on_current_platform() {
        assert!(current_target().is_ok());
    }

    #[test]
    fn replace_executable_stages_and_swaps_file() {
        let dir = std::env::temp_dir().join(format!("floway-replace-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("dummy-binary");
        std::fs::write(&target, b"original-content").unwrap();

        let new_content = b"new-binary-content";
        replace_executable(&target, new_content).unwrap();

        let read_back = std::fs::read(&target).unwrap();
        assert_eq!(read_back, new_content);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
