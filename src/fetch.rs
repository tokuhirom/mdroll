//! Fetching remote images.
//!
//! A README points at its logo and its badges over https, so a viewer that only
//! reads the disk shows a wall of alt text. Downloads land in a cache keyed by
//! URL, which makes the second look at a document instant and offline, and they
//! happen off the UI thread, because a slow host must never hold up a redraw.

use crate::cache;
use anyhow::{Result, bail};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// Long enough for a slow CDN, short enough that a dead host does not keep a
/// worker thread alive for the rest of the session.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Enough for any logo or screenshot. A document pointing at something far
/// larger than this is not worth the wait, and it would be scaled down to a few
/// hundred pixels anyway.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Whether a URL is something to go and fetch.
///
/// Deliberately narrow: `data:` and `file:` URLs, and anything else exotic, are
/// left to render as alt text rather than handed to an HTTP client.
pub fn is_remote(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Where a URL's download lives, whether or not it has happened yet.
///
/// No extension: what the bytes are is decided by looking at them, since a
/// badge URL carries no file name to go on.
pub fn cache_path(url: &str) -> Option<PathBuf> {
    Some(cache::dir("images")?.join(format!("{:016x}", cache::key(&[url]))))
}

/// The cached file for `url`, if it has already been downloaded.
pub fn cached(url: &str) -> Option<PathBuf> {
    cache_path(url).filter(|path| path.is_file())
}

/// Download `url` into the cache and return the file it landed in.
///
/// Blocking, so callers run it on a worker thread.
pub fn fetch(url: &str) -> Result<PathBuf> {
    let Some(path) = cache_path(url) else {
        bail!("no cache directory available");
    };
    if path.is_file() {
        return Ok(path);
    }
    cache::make_dir("images")?;

    let mut response = agent().get(url).call()?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()?;
    if body.is_empty() {
        bail!("empty response");
    }

    // Written under a temporary name and renamed into place, so a download that
    // is interrupted — or one racing another mdroll — never leaves a truncated
    // file behind to be taken for a cache hit. Narrowed before the rename, so
    // it is never briefly readable under its final name.
    let temp = path.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&temp, &body)?;
    cache::restrict(&temp)?;
    std::fs::rename(&temp, &path)?;
    Ok(path)
}

/// The shared HTTP client. Cloning it shares the connection pool, which is what
/// makes a row of badges from one host cost roughly one round trip.
fn agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(TIMEOUT))
                .user_agent(concat!("mdroll/", env!("CARGO_PKG_VERSION")))
                .build()
                .into()
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_are_fetched() {
        assert!(is_remote("https://example.com/a.png"));
        assert!(is_remote("HTTP://example.com/a.png"));
        assert!(!is_remote("data:image/png;base64,AAAA"));
        assert!(!is_remote("file:///tmp/a.png"));
        assert!(!is_remote("./local.png"));
    }

    #[test]
    fn each_url_gets_its_own_cache_file() {
        let Some(a) = cache_path("https://example.com/a.png") else {
            return;
        };
        assert_ne!(Some(&a), cache_path("https://example.com/b.png").as_ref());
        assert_eq!(Some(&a), cache_path("https://example.com/a.png").as_ref());
        assert!(a.starts_with(cache::dir("images").unwrap()));
    }

    #[test]
    fn a_url_that_was_never_fetched_has_no_cached_file() {
        assert!(cached("https://example.invalid/never-fetched-8ac1f.png").is_none());
    }
}
