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

    // Create a FETCH_HEAD marker so `cache_age_seconds` has a
    // canonical "last network refresh" mtime to read. Without this,
    // age is computed from `.git/HEAD` which can be inadvertently
    // touched by `git checkout` / `git reset` inside the cache.
    let fetch_head = cache.join(".git").join("FETCH_HEAD");
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&fetch_head)
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to create FETCH_HEAD marker at {}: {e}",
                fetch_head.display()
            )
        })?;

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

/// Age of the ecc cache in seconds. Uses the mtime of `.git/FETCH_HEAD`
/// or `.git/HEAD`, whichever is newer. Returns None when the cache
/// doesn't exist.
pub fn cache_age_seconds(config_dir: &std::path::Path) -> Option<u64> {
    let cache = cache_dir(config_dir);
    let candidates = [cache.join(".git/FETCH_HEAD"), cache.join(".git/HEAD")];
    let newest = candidates
        .iter()
        .filter_map(|p| p.metadata().ok())
        .filter_map(|m| m.modified().ok())
        .max()?;
    let now = std::time::SystemTime::now();
    now.duration_since(newest).ok().map(|d| d.as_secs())
}

/// If `cfg.auto_refresh` is true AND the cache is older than `refresh_ttl`,
/// spawn a thread-based background `git fetch + reset --hard` and return
/// immediately. No-op when cache is fresh, missing, or auto_refresh is off.
///
/// CAUTION: this mutates the cache directory in the background. Callers
/// that need to read ecc files from the cache in THIS run MUST complete
/// those reads BEFORE calling this function, or copy the files to a
/// staging directory first.
///
/// This function is only invoked from the template-mode code path in
/// `prepare_context`, after staging assembly completes. Host-mode runs
/// (no `--template`, no `--lang`, no cwd lang detection) do not read
/// from the ecc cache, so they also do not trigger refreshes —
/// intentional, not a bug.
pub fn maybe_background_refresh(config_dir: &std::path::Path, cfg: &crate::config::EccConfig) {
    if !cfg.auto_refresh {
        return;
    }
    let age = match cache_age_seconds(config_dir) {
        Some(a) => a,
        None => return,
    };
    if age < cfg.refresh_ttl_seconds() {
        return;
    }

    let cache = cache_dir(config_dir);
    let reference = cfg.r#ref.clone();

    // Fire-and-forget: spawn a background thread that runs fetch + reset
    // via direct `Command` invocations. No shell involved — safer (no
    // escape edge cases) and no dependency on `/bin/sh`. The thread dies
    // with the process, which is fine: the TTL will trigger another
    // attempt on the next vibepod invocation if this one was killed mid-flight.
    std::thread::spawn(move || {
        let fetch_status = std::process::Command::new("git")
            .current_dir(&cache)
            .args(["fetch", "--depth", "1", "origin", &reference])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if matches!(fetch_status, Ok(s) if s.success()) {
            let _ = std::process::Command::new("git")
                .current_dir(&cache)
                .args(["reset", "--hard", "FETCH_HEAD"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    });
}

/// Copy selected ecc files from the cache to the staging directory.
///
/// Skills keep their directory structure under `skills/`:
/// `skills/<name>/SKILL.md` → `<staging>/skills/<name>/SKILL.md`
/// Agents are flat files under `agents/`:
/// `agents/<name>.md` → `<staging>/agents/<name>.md`
///
/// The staging layout matches the template-root layout (not `.claude/`);
/// the `.claude/` mount paths are applied at mount time in
/// `build_template_mounts`, not at stage time.
///
/// Callers must have previously validated `selection` via the template
/// metadata parser (no absolute paths, no `..`, no empty strings).
/// This function assumes structural safety and only guards against
/// missing source files.
///
/// Fails fast if any listed file is missing in the cache.
pub fn stage_files(
    config_dir: &std::path::Path,
    runtime_dir: &std::path::Path,
    selection: &crate::cli::run::template::EccSelection,
) -> Result<()> {
    let cache = cache_dir(config_dir);
    let staging = staging_dir(runtime_dir);
    copy_selection(
        &cache,
        &staging.join("skills"),
        "skill",
        "skills/",
        &selection.skills,
    )?;
    copy_selection(
        &cache,
        &staging.join("agents"),
        "agent",
        "agents/",
        &selection.agents,
    )?;
    Ok(())
}

fn copy_selection(
    cache: &std::path::Path,
    dest_root: &std::path::Path,
    kind: &str,
    prefix: &str,
    entries: &[String],
) -> Result<()> {
    for rel in entries {
        let src = cache.join(rel);
        if !src.is_file() {
            anyhow::bail!(
                "ecc {kind} not found in cache: '{}' (expected at {})",
                rel,
                src.display()
            );
        }
        let stripped = rel.strip_prefix(prefix).ok_or_else(|| {
            anyhow::anyhow!("ecc {kind} path '{}' must start with '{}'", rel, prefix)
        })?;
        let dest = dest_root.join(stripped);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
        }
        std::fs::copy(&src, &dest).map_err(|e| {
            anyhow::anyhow!(
                "failed to copy {} to {}: {e}",
                src.display(),
                dest.display()
            )
        })?;
    }
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
    fn cache_age_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(cache_age_seconds(dir.path()).is_none());
    }

    #[test]
    fn cache_age_reflects_head_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_dir(dir.path());
        std::fs::create_dir_all(cache.join(".git")).unwrap();
        std::fs::write(cache.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let age = cache_age_seconds(dir.path()).unwrap();
        assert!(age < 5, "fresh file should be age < 5s, got {age}");
    }

    #[test]
    fn maybe_background_refresh_noop_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::EccConfig {
            auto_refresh: false,
            ..Default::default()
        };
        // Should return without panicking, without spawning anything observable.
        maybe_background_refresh(dir.path(), &cfg);
    }

    #[test]
    fn maybe_background_refresh_noop_when_cache_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::EccConfig::default();
        // No `.git/FETCH_HEAD` or `.git/HEAD`, so `cache_age_seconds` → None
        // and the function returns without trying to spawn.
        maybe_background_refresh(dir.path(), &cfg);
    }

    #[test]
    fn stage_files_copies_selected_skill_and_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&runtime_dir).unwrap();

        // Fake ecc-cache layout
        let cache = cache_dir(&config_dir);
        std::fs::create_dir_all(cache.join("skills/rust-patterns")).unwrap();
        std::fs::write(
            cache.join("skills/rust-patterns/SKILL.md"),
            "# Rust Patterns",
        )
        .unwrap();
        std::fs::create_dir_all(cache.join("agents")).unwrap();
        std::fs::write(cache.join("agents/rust-reviewer.md"), "# Rust Reviewer").unwrap();

        let sel = crate::cli::run::template::EccSelection {
            skills: vec!["skills/rust-patterns/SKILL.md".to_string()],
            agents: vec!["agents/rust-reviewer.md".to_string()],
        };
        stage_files(&config_dir, &runtime_dir, &sel).unwrap();

        let staging = staging_dir(&runtime_dir);
        let skill_out = staging.join("skills/rust-patterns/SKILL.md");
        assert!(
            skill_out.is_file(),
            "skill should be staged at {}",
            skill_out.display()
        );
        assert_eq!(
            std::fs::read_to_string(&skill_out).unwrap(),
            "# Rust Patterns"
        );

        let agent_out = staging.join("agents/rust-reviewer.md");
        assert!(agent_out.is_file());
        assert_eq!(
            std::fs::read_to_string(&agent_out).unwrap(),
            "# Rust Reviewer"
        );
    }

    #[test]
    fn stage_files_fails_when_skill_missing_in_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::create_dir_all(cache_dir(&config_dir)).unwrap();

        let sel = crate::cli::run::template::EccSelection {
            skills: vec!["skills/nonexistent/SKILL.md".to_string()],
            agents: vec![],
        };
        let err = stage_files(&config_dir, &runtime_dir, &sel).unwrap_err();
        assert!(
            format!("{err}").contains("skills/nonexistent"),
            "expected missing-file error mentioning path, got: {err}"
        );
    }

    #[test]
    fn stage_files_fails_when_agent_missing_in_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::create_dir_all(cache_dir(&config_dir)).unwrap();

        let sel = crate::cli::run::template::EccSelection {
            skills: vec![],
            agents: vec!["agents/missing.md".to_string()],
        };
        let err = stage_files(&config_dir, &runtime_dir, &sel).unwrap_err();
        assert!(
            format!("{err}").contains("agents/missing.md"),
            "expected missing-file error mentioning agent path, got: {err}"
        );
    }

    #[test]
    fn stage_files_noop_when_selection_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&runtime_dir).unwrap();

        let sel = crate::cli::run::template::EccSelection::default();
        stage_files(&config_dir, &runtime_dir, &sel).unwrap();

        assert!(
            !staging_dir(&runtime_dir).exists(),
            "empty selection should not create staging directory"
        );
    }

    #[test]
    fn stage_files_leaves_prior_entries_staged_when_later_entry_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&runtime_dir).unwrap();

        let cache = cache_dir(&config_dir);
        std::fs::create_dir_all(cache.join("skills/first")).unwrap();
        std::fs::write(cache.join("skills/first/SKILL.md"), "first").unwrap();
        // Note: skills/missing/SKILL.md is intentionally NOT created

        let sel = crate::cli::run::template::EccSelection {
            skills: vec![
                "skills/first/SKILL.md".to_string(),
                "skills/missing/SKILL.md".to_string(),
            ],
            agents: vec![],
        };
        let err = stage_files(&config_dir, &runtime_dir, &sel).unwrap_err();
        assert!(format!("{err}").contains("skills/missing"));

        // First entry was copied before the failure; we do NOT roll it back.
        let first_staged = staging_dir(&runtime_dir).join("skills/first/SKILL.md");
        assert!(
            first_staged.is_file(),
            "first entry should remain staged on fail-fast; found: {}",
            first_staged.display()
        );
    }

    #[test]
    fn stage_files_preserves_nested_skill_directory_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&runtime_dir).unwrap();

        let cache = cache_dir(&config_dir);
        std::fs::create_dir_all(cache.join("skills/nested/deep")).unwrap();
        std::fs::write(cache.join("skills/nested/deep/SKILL.md"), "deep").unwrap();

        let sel = crate::cli::run::template::EccSelection {
            skills: vec!["skills/nested/deep/SKILL.md".to_string()],
            agents: vec![],
        };
        stage_files(&config_dir, &runtime_dir, &sel).unwrap();

        let out = staging_dir(&runtime_dir).join("skills/nested/deep/SKILL.md");
        assert!(out.is_file(), "nested skill path should be preserved");
    }
}
