//! ANSI colour helpers and stdin prompts. Respects NO_COLOR and non-TTY.

use anyhow::{Context, Result};
use std::io::{BufRead, IsTerminal, Write};

fn colour(code: &str, text: &str) -> String {
    if std::env::var("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
        || !std::io::stdout().is_terminal()
    {
        return text.to_string();
    }
    format!("\x1b[{code}m{text}\x1b[0m")
}

pub fn green(text: &str) -> String {
    colour("32", text)
}

pub fn red(text: &str) -> String {
    colour("31", text)
}

pub fn cyan(text: &str) -> String {
    colour("36", text)
}

pub fn dim(text: &str) -> String {
    colour("2", text)
}

pub fn bold(text: &str) -> String {
    colour("1", text)
}

pub fn flush() {
    let _ = std::io::stdout().flush();
}

/// Read a line from stdin, showing `default` when the user presses Enter.
pub fn prompt(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("could not read from stdin")?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Read a secret-ish value without echo; falls back to echo when termios is
/// unavailable.
pub fn secret_prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    flush();
    let masked = read_masked()?;
    if !masked.is_empty() {
        println!();
        return Ok(masked);
    }
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("could not read from stdin")?;
    Ok(line.trim().to_string())
}

/// Prompt with a saved default; Enter accepts the saved value.
pub fn secret_prompt_with_default(label: &str, default: &str) -> Result<String> {
    print!("{label} [press Enter to reuse the saved key]: ");
    flush();
    let masked = read_masked()?;
    if !masked.is_empty() {
        println!();
        return Ok(masked);
    }
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("could not read from stdin")?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Raw-read bytes with echo disabled. Empty result means "echo fell back" or
/// "user typed nothing".
#[cfg(unix)]
fn read_masked() -> Result<String> {
    let mut termios = std::mem::MaybeUninit::uninit();
    // SAFETY: termios(3) — tcgetattr fills the termios struct; the fd is stdin.
    if unsafe { libc::tcgetattr(0, termios.as_mut_ptr()) } != 0 {
        return Ok(String::new());
    }
    let mut termios = unsafe { termios.assume_init() };
    let original = termios;
    termios.c_lflag &= !libc::ECHO;
    // SAFETY: same termios struct we just read.
    if unsafe { libc::tcsetattr(0, libc::TCSANOW, &termios) } != 0 {
        return Ok(String::new());
    }
    let mut buffer = Vec::new();
    let result = std::io::stdin().lock().read_until(b'\n', &mut buffer);
    // Restore echo before anything else.
    // SAFETY: restoring the saved attributes.
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &original) };
    result.context("could not read from stdin")?;
    let text = String::from_utf8_lossy(&buffer);
    Ok(text.trim().to_string())
}

#[cfg(not(unix))]
fn read_masked() -> Result<String> {
    Ok(String::new())
}
