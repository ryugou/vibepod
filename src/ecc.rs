//! ECC (everything-claude-code) cache management.
//!
//! Clones / fetches the ecc repository to `~/.config/vibepod/ecc-cache/`
//! and stages per-run subsets of its files for container mounts.

use anyhow::Result;
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

/// Ensure `cache_dir(config_dir)` contains a clone of `cfg.repo` at `cfg.ref`.
///
/// If the cache already has a `.git` directory, this is a no-op and returns Ok.
/// Otherwise performs `git clone --depth 1 --branch <ref> <repo> <cache>`.
///
/// Removes any partial cache directory left over from a previous failed clone
/// (presence without `.git` = incomplete).
///
/// Idempotent: safe to call multiple times.
pub fn ensure_cloned(config_dir: &std::path::Path, cfg: &crate::config::EccConfig) -> Result<()> {
    let cache = cache_dir(config_dir);

    if cache.join(".git").is_dir() {
        return Ok(());
    }

    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!(
                "failed to create parent directory {}: {e}",
                parent.display()
            )
        })?;
    }

    // Remove any half-populated directory from a prior failed clone.
    if cache.exists() {
        std::fs::remove_dir_all(&cache).map_err(|e| {
            anyhow::anyhow!(
                "failed to remove incomplete cache at {}: {e}",
                cache.display()
            )
        })?;
    }

    let output = std::process::Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg(&cfg.r#ref)
        .arg(&cfg.repo)
        .arg(&cache)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to spawn git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git clone failed ({}): {}", output.status, stderr.trim());
    }

    Ok(())
}

/// Fetch and hard-reset the ecc cache to `cfg.ref`. Caller must ensure
/// `ensure_cloned` has been run first.
///
/// Errors if the cache directory has no `.git` or if any git command fails.
pub fn fetch_latest(config_dir: &std::path::Path, cfg: &crate::config::EccConfig) -> Result<()> {
    let cache = cache_dir(config_dir);
    if !cache.join(".git").is_dir() {
        anyhow::bail!(
            "ecc cache not initialized at {}: run `vibepod init` first",
            cache.display()
        );
    }

    let run = |args: &[&str]| -> Result<()> {
        let output = std::process::Command::new("git")
            .current_dir(&cache)
            .args(args)
            .output()
            .map_err(|e| anyhow::anyhow!("failed to spawn git: {e}"))?;
        if !output.status.success() {
            anyhow::bail!(
                "git {} failed ({}): {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    };

    run(&["fetch", "--depth", "1", "origin", &cfg.r#ref])?;
    run(&["reset", "--hard", "FETCH_HEAD"])?;
    Ok(())
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
    fn ensure_cloned_noop_when_git_dir_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path();
        let cache = cache_dir(config_dir);
        std::fs::create_dir_all(cache.join(".git")).unwrap();
        std::fs::write(cache.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let cfg = crate::config::EccConfig::default();
        ensure_cloned(config_dir, &cfg).unwrap();

        // Verify existing .git/HEAD was not overwritten (confirms no git operation happened).
        let head = std::fs::read_to_string(cache.join(".git/HEAD")).unwrap();
        assert_eq!(head, "ref: refs/heads/main\n");
    }

    #[test]
    fn fetch_latest_errors_when_cache_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::EccConfig::default();
        let err = fetch_latest(dir.path(), &cfg).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("vibepod init"),
            "expected init hint in error, got: {msg}"
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
