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
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// The subdirectories this module manages.
const KINDS: &[&str] = &["images", "mermaid"];

/// How long an entry is kept.
///
/// A cache hit does not touch the file, so this is time since it was written,
/// not time since it was last wanted. That is the point: a badge whose image
/// changes is otherwise pinned to the first version ever fetched, and a week is
/// short enough that a stale build status corrects itself while still being far
/// longer than a reading session.
pub const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A named subdirectory of the user's cache directory.
pub fn dir(kind: &str) -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("mdroll").join(kind))
}

/// Once-per-run housekeeping: narrow what is already there, and drop what has
/// expired.
///
/// Narrowing has to happen even in a run that caches nothing new, because a
/// directory left open by an earlier version would otherwise stay open forever.
/// Neither job creates a directory: a run that never caches anything should not
/// leave one behind.
pub fn prepare() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        for kind in KINDS {
            if dir(kind).is_some_and(|dir| dir.is_dir()) {
                let _ = make_dir(kind);
                sweep(kind, MAX_AGE);
            }
        }
    });
}

/// Delete entries under `kind` last written more than `max_age` ago.
///
/// Best-effort throughout. A cache is by definition reconstructible, so a file
/// that cannot be read or removed — another `mdroll` holding it, a filesystem
/// with no timestamps — is left where it is rather than reported.
pub fn sweep(kind: &str, max_age: Duration) {
    let Some(dir) = dir(kind) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        // A clock that has gone backwards since the file was written yields an
        // error here, which counts as "not old enough" and leaves it alone.
        let expired = metadata
            .modified()
            .ok()
            .and_then(|written| now.duration_since(written).ok())
            .is_some_and(|age| age > max_age);
        if expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

    #[test]
    fn sweeping_removes_what_has_expired_and_keeps_what_has_not() {
        let kind = format!("test-sweep-{}", std::process::id());
        let Ok(dir) = make_dir(&kind) else {
            return;
        };
        let (fresh, stale) = (dir.join("fresh"), dir.join("stale"));
        std::fs::write(&fresh, b"x").unwrap();
        std::fs::write(&stale, b"x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(SystemTime::now() - MAX_AGE - Duration::from_secs(60))
            .unwrap();

        sweep(&kind, MAX_AGE);

        let (kept, gone) = (fresh.is_file(), !stale.exists());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kept, "an entry written recently stays");
        assert!(gone, "an entry older than the maximum age goes");
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
