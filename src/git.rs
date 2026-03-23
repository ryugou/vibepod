use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn get_head_hash(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .context("Failed to get HEAD hash")?;
    if !output.status.success() {
        anyhow::bail!("Failed to get HEAD hash");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn get_current_branch(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(path)
        .output()
        .context("Failed to get current branch")?;
    if !output.status.success() {
        anyhow::bail!("Failed to get current branch");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn get_remote_url(path: &Path) -> Option<String> {
    Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(path)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}

pub fn commit_exists(path: &Path, hash: &str) -> bool {
    Command::new("git")
        .args(["cat-file", "-t", hash])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_ancestor(path: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn has_uncommitted_changes(path: &Path) -> bool {
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output();
    match status {
        Ok(o) => !o.stdout.is_empty(),
        Err(_) => false,
    }
}

pub fn get_commit_log(path: &Path, from: &str, to: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["log", "--oneline", &format!("{}..{}", from, to)])
        .current_dir(path)
        .output()
        .context("Failed to get commit log")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn get_diff_stat(path: &Path, from: &str, to: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "--stat", &format!("{}..{}", from, to)])
        .current_dir(path)
        .output()
        .context("Failed to get diff stat")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn get_changed_files(path: &Path, from: &str, to: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "--name-status", &format!("{}..{}", from, to)])
        .current_dir(path)
        .output()
        .context("Failed to get changed files")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn get_untracked_files(path: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["clean", "-fdn"])
        .current_dir(path)
        .output()
        .context("Failed to list untracked files")?;
    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim_start_matches("Would remove ").to_string())
        .collect();
    Ok(files)
}

pub fn reset_hard(path: &Path, commit: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["reset", "--hard", commit])
        .current_dir(path)
        .output()
        .context("Failed to git reset --hard")?;
    if !output.status.success() {
        anyhow::bail!(
            "git reset --hard failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn clean_fd(path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["clean", "-fd"])
        .current_dir(path)
        .output()
        .context("Failed to git clean -fd")?;
    if !output.status.success() {
        anyhow::bail!(
            "git clean -fd failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
