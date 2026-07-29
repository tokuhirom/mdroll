//! Where results too expensive to recompute are kept between runs.
//!
//! Two things qualify: diagrams rendered by `mmdc`, which cost a browser
//! launch, and images fetched over the network, which cost a round trip.
//!
//! Everything here is kept readable only by its owner. A cache is a record of
//! which documents have been opened and what they pointed at, and on a shared
//! machine that is nobody else's business.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// A named subdirectory of the user's cache directory.
pub fn dir(kind: &str) -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("mdroll").join(kind))
}

/// Create a cache subdirectory, owner-only, and return it.
///
/// Both it and the `mdroll` directory above it are narrowed, so a cache that
/// predates this — or one whose parent was created by an earlier version — is
/// closed on the next run rather than staying open forever.
pub fn make_dir(kind: &str) -> Result<PathBuf> {
    let dir = dir(kind).context("no cache directory available")?;
    std::fs::create_dir_all(&dir)?;
    if let Some(parent) = dir.parent() {
        set_mode(parent, 0o700)?;
    }
    set_mode(&dir, 0o700)?;
    Ok(dir)
}

/// Narrow a cache file to its owner, before it is put where readers look.
pub fn restrict(path: &Path) -> Result<()> {
    set_mode(path, 0o600)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("restricting {}", path.display()))?;
    Ok(())
}

/// Windows has no mode bits to set; the profile directory is already
/// per-user.
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// FNV-1a over the parts, with a separator between them.
///
/// The key only has to be stable across runs of the same binary, which is
/// exactly what `DefaultHasher` does not promise — its output is explicitly
/// allowed to change between Rust releases.
pub fn key(parts: &[&str]) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_stable_and_separates_its_parts() {
        assert_eq!(key(&["ab", "c"]), key(&["ab", "c"]));
        // Without a separator these would collide.
        assert_ne!(key(&["ab", "c"]), key(&["a", "bc"]));
    }

    #[test]
    fn each_kind_gets_its_own_directory() {
        let Some(images) = dir("images") else {
            return;
        };
        assert_ne!(Some(images), dir("mermaid"));
    }

    #[cfg(unix)]
    #[test]
    fn a_restricted_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("mdroll-restrict-{}", std::process::id()));
        std::fs::write(&path, b"secret").unwrap();
        restrict(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_cache_directory_and_its_parent_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        // Named for this process, so a parallel test run cannot collide.
        let kind = format!("test-{}", std::process::id());
        let Ok(dir) = make_dir(&kind) else {
            return;
        };
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let (own, parent) = (mode(&dir), mode(dir.parent().unwrap()));
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(own, 0o700);
        assert_eq!(parent, 0o700, "the mdroll directory above it too");
    }
}
