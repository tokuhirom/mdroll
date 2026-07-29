//! Rendering mermaid diagrams through `mmdc`, the mermaid CLI.
//!
//! The box-drawing renderer in [`crate::mermaid`] covers flowcharts and
//! sequence diagrams and is instant, selectable text. `mmdc` covers everything
//! else, at the cost of launching a headless browser — so results are cached on
//! disk by content hash, and the work happens off the UI thread.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate the `mmdc` binary. `MDROLL_MMDC` overrides the search.
pub fn binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MDROLL_MMDC") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    which("mmdc")
}

pub fn available() -> bool {
    binary().is_some()
}

/// Minimal `which`, to avoid a dependency for one lookup.
///
/// `mmdc` is a Node shim, which on Windows means `mmdc.cmd` and not a bare
/// `mmdc`, so each of `PATHEXT`'s suffixes is tried as well as the name on its
/// own. On Unix a candidate only counts if it is executable: a data file that
/// happens to share the name is not the binary.
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let base = dir.join(name);
        if is_executable(&base) {
            return Some(base);
        }
        for ext in extensions() {
            let mut suffixed = base.clone().into_os_string();
            suffixed.push(&ext);
            let candidate = PathBuf::from(suffixed);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Suffixes a bare command name may carry. Nothing outside Windows.
#[cfg(windows)]
fn extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(not(windows))]
fn extensions() -> Vec<String> {
    Vec::new()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Windows decides from the extension, which the search has already matched.
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub fn cache_dir() -> Option<PathBuf> {
    crate::cache::dir("mermaid")
}

pub fn cache_path(code: &str, dark: bool) -> Option<PathBuf> {
    let theme = if dark { "dark" } else { "default" };
    let key = crate::cache::key(&[code, theme]);
    Some(cache_dir()?.join(format!("{key:016x}-{theme}.png")))
}

/// Render `code` to a PNG, returning the cached path.
///
/// Blocking and slow the first time — a browser has to start — so callers run
/// this off the UI thread. Subsequent calls for the same diagram are a stat.
pub fn render(code: &str, dark: bool) -> Result<PathBuf> {
    let Some(output) = cache_path(code, dark) else {
        bail!("no cache directory available");
    };
    if output.is_file() {
        return Ok(output);
    }
    let Some(mmdc) = binary() else {
        bail!("mmdc is not installed");
    };
    // Owner-only: the diagram source goes through here, and the rendered PNG
    // says as much about the document as the source does.
    let dir = crate::cache::make_dir("mermaid")?;

    let input = dir.join(format!("{}.mmd", std::process::id()));
    std::fs::write(&input, code)?;
    crate::cache::restrict(&input)?;
    let result = run(&mmdc, &input, &output, dark);
    let _ = std::fs::remove_file(&input);
    result?;

    if !output.is_file() {
        bail!("mmdc reported success but wrote nothing");
    }
    // `mmdc` wrote it, so its mode is whatever that process's umask allowed.
    crate::cache::restrict(&output)?;
    Ok(output)
}

fn run(mmdc: &Path, input: &Path, output: &Path, dark: bool) -> Result<()> {
    let theme = if dark { "dark" } else { "default" };
    let attempt = |extra: Option<&Path>| -> Result<std::process::Output> {
        let mut command = Command::new(mmdc);
        command
            .arg("-i")
            .arg(input)
            .arg("-o")
            .arg(output)
            // Transparent, so the terminal's own background shows through the
            // diagram exactly as it does behind the box-drawing renderer.
            .args(["-b", "transparent", "-t", theme, "-s", "3", "-q"]);
        if let Some(config) = extra {
            command.arg("-p").arg(config);
        }
        Ok(command.output()?)
    };

    let first = attempt(None)?;
    if first.status.success() {
        return Ok(());
    }

    // Chrome's sandbox needs user namespaces, which containers and some
    // hardened kernels do not give it. Only disable it after the sandboxed
    // attempt has actually failed.
    let config = output.with_file_name("puppeteer.json");
    std::fs::write(&config, br#"{"args":["--no-sandbox","--disable-gpu"]}"#)?;
    let second = attempt(Some(&config))?;
    if second.status.success() {
        return Ok(());
    }
    bail!(
        "mmdc failed: {}",
        String::from_utf8_lossy(&second.stderr)
            .lines()
            .find(|l| l.contains("Error") || l.contains("error"))
            .unwrap_or("no diagnostic")
            .trim()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_key_depends_on_the_code_and_the_theme() {
        let a = cache_path("flowchart TD\n A-->B", true);
        let b = cache_path("flowchart TD\n A-->C", true);
        let c = cache_path("flowchart TD\n A-->B", false);
        assert_ne!(a, b, "different diagrams must not share a file");
        assert_ne!(a, c, "different themes must not share a file");
        assert_eq!(a, cache_path("flowchart TD\n A-->B", true));
    }

    #[test]
    fn cache_paths_are_png_files_under_the_cache_directory() {
        let Some(path) = cache_path("x", true) else {
            return;
        };
        assert_eq!(path.extension().unwrap(), "png");
        assert!(path.starts_with(cache_dir().unwrap()));
    }

    #[test]
    fn which_finds_a_binary_that_is_certainly_on_the_path() {
        // `sh` is on PATH on every platform this runs on except Windows.
        if cfg!(unix) {
            assert!(which("sh").is_some());
        }
        assert!(which("mdroll-definitely-not-a-real-binary").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_is_not_executable_is_not_the_binary() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("mdroll-notexec-{}", std::process::id()));
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let readable_only = is_executable(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runnable = is_executable(&path);
        let _ = std::fs::remove_file(&path);
        assert!(!readable_only, "0644 is not something to run");
        assert!(runnable, "0755 is");
    }
}
