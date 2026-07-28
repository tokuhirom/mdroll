//! Rendering mermaid diagrams through `mmdc`, the mermaid CLI.
//!
//! The box-drawing renderer in [`crate::mermaid`] covers flowcharts and
//! sequence diagrams and is instant, selectable text. `mmdc` covers everything
//! else, at the cost of launching a headless browser — so results are cached on
//! disk by content hash, and the work happens off the UI thread.

use anyhow::{Context, Result, bail};
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
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("mdroll").join("mermaid"))
}

/// FNV-1a. The cache key only has to be stable across runs of the same
/// binary, which rules out `DefaultHasher` — its output is explicitly not
/// guaranteed between Rust releases.
fn hash(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.as_bytes() {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn cache_path(code: &str, dark: bool) -> Option<PathBuf> {
    let theme = if dark { "dark" } else { "default" };
    let key = hash(&[code, theme]);
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
    let dir = output.parent().context("cache path has no parent")?;
    std::fs::create_dir_all(dir)?;

    let input = dir.join(format!("{}.mmd", std::process::id()));
    std::fs::write(&input, code)?;
    let result = run(&mmdc, &input, &output, dark);
    let _ = std::fs::remove_file(&input);
    result?;

    if !output.is_file() {
        bail!("mmdc reported success but wrote nothing");
    }
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
    fn the_hash_is_stable_and_separates_its_parts() {
        assert_eq!(hash(&["ab", "c"]), hash(&["ab", "c"]));
        // Without a separator these would collide.
        assert_ne!(hash(&["ab", "c"]), hash(&["a", "bc"]));
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
}
