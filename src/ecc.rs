//! ECC (everything-claude-code) cache management.
//!
//! Clones / fetches the ecc repository to `~/.config/vibepod/ecc-cache/`
//! and stages per-run subsets of its files for container mounts.

use std::path::PathBuf;

/// ECC cache root: `~/.config/vibepod/ecc-cache/`.
pub fn cache_dir(config_dir: &std::path::Path) -> PathBuf {
    config_dir.join("ecc-cache")
}

/// Per-container staging directory:
/// `<runtime_dir>/ecc-staging/`. `runtime_dir` is
/// `~/.config/vibepod/runtime/<container_name>/`.
pub fn staging_dir(runtime_dir: &std::path::Path) -> PathBuf {
    runtime_dir.join("ecc-staging")
}

/// Advisory lock file used to serialize ecc-cache mutations
/// (clone / fetch / reset). Helps prevent two concurrent
/// `vibepod run` from stepping on each other.
pub fn lock_file_path(config_dir: &std::path::Path) -> PathBuf {
    config_dir.join("ecc-cache.lock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn cache_dir_under_config_dir() {
        let cfg = Path::new("/tmp/vibepod");
        assert_eq!(cache_dir(cfg), Path::new("/tmp/vibepod/ecc-cache"));
    }

    #[test]
    fn staging_dir_under_runtime_dir() {
        let rt = Path::new("/tmp/vibepod/runtime/foo");
        assert_eq!(
            staging_dir(rt),
            Path::new("/tmp/vibepod/runtime/foo/ecc-staging")
        );
    }

    #[test]
    fn lock_file_path_under_config_dir() {
        let cfg = Path::new("/tmp/vibepod");
        assert_eq!(
            lock_file_path(cfg),
            Path::new("/tmp/vibepod/ecc-cache.lock")
        );
    }
}
