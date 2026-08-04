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
                let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
                Some(sanitize_remote_url(&url))
            } else {
                None
            }
        })
}

/// Remove credentials from remote URLs (e.g., x-access-token:gho_xxx@)
fn sanitize_remote_url(url: &str) -> String {
    if let Some(at_pos) = url.find('@') {
        if let Some(proto_end) = url.find("://") {
            let prefix = &url[..proto_end + 3];
            let after_at = &url[at_pos + 1..];
            return format!("{}{}", prefix, after_at);
        }
    }
    url.to_string()
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

/// `has_uncommitted_changes` の probe 失敗を区別できる版。
///
/// Copilot 指摘 + reviewer mn2: `has_uncommitted_changes` は spawn 失敗・
/// git 非ゼロ終了（非 git ディレクトリ等）を区別せず `false`（clean）に
/// 潰す。`restore.rs` はこの関数を「bail するかどうか」の判定にしか使って
/// おらず、probe 失敗を「変更なし」寄りに倒しても実害が無い（bail しない
/// 方向へ倒れるだけ）ため、`has_uncommitted_changes` 自体の既存挙動は
/// 変更しない。
///
/// 一方タイムアウト案内（`render_timeout_message`）は probe 失敗を
/// 「clean」と誤断定すると、実際には変更が残っているかもしれない
/// workspace に対して `vibepod restore` や `--force` 無しの
/// `git worktree remove` を安全であるかのように勧めてしまう危険がある。
/// そのため probe 失敗を `None` として明示的に区別できるこの関数を別途
/// 用意する。
///
/// `Some(true)`: 未コミットの変更あり。`Some(false)`: クリーン。
/// `None`: spawn 失敗、または `git` が非ゼロ終了した（probe 自体が信頼
/// できない）。
pub fn try_has_uncommitted_changes(path: &Path) -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}

pub fn get_commit_log(path: &Path, from: &str, to: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["log", "--oneline", &format!("{}..{}", from, to)])
        .current_dir(path)
        .output()
        .context("Failed to get commit log")?;
    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn get_diff_stat(path: &Path, from: &str, to: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "--stat", &format!("{}..{}", from, to)])
        .current_dir(path)
        .output()
        .context("Failed to get diff stat")?;
    if !output.status.success() {
        anyhow::bail!(
            "git diff --stat failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn get_changed_files(path: &Path, from: &str, to: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "--name-status", &format!("{}..{}", from, to)])
        .current_dir(path)
        .output()
        .context("Failed to get changed files")?;
    if !output.status.success() {
        anyhow::bail!(
            "git diff --name-status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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

/// 実行後の要約で「エージェントが触ったファイル一覧」をどう算出できたか。
///
/// `Computed(空 Vec)`（本当に無変更）と `Unavailable`（git が失敗して
/// 算出不能）を**明確に区別する**ための型。両者を空 Vec に潰すと、算出
/// 失敗が「変更なし」に化けて呼び出し元セッションが誤判断するため。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedFiles {
    /// すべての git 呼び出しが成功した。空でも「本当に無変更」を意味する。
    /// 重複除去・安定ソート済み。
    Computed(Vec<String>),
    /// git 呼び出しの少なくとも 1 つが spawn 失敗または非ゼロ終了したため、
    /// 一覧が不明。中途半端に拾えた分は「完全な一覧」と誤認させないよう破棄する。
    Unavailable,
}

/// セッション開始時のコミット `since` から見て、エージェントが触った
/// ファイルの一覧を集める（表示用のベストエフォート）。
///
/// 2 つのソースを和集合にする:
/// 1. `git diff --name-only <since>` — `since` と現在の作業ツリーの差分。
///    コミット済みの変更も未コミットの変更も 1 度で拾える（tracked のみ）。
/// 2. `git ls-files --others --exclude-standard` — 新規の未追跡ファイル。
///
/// これは要約表示専用のため git が失敗しても致命扱いにはしないが、失敗を
/// **握りつぶさず** `ChangedFiles::Unavailable` として返す。どちらかの
/// コマンドが失敗したら、部分的に拾えた分は破棄して `Unavailable` を返す
/// （不完全な一覧を「完全」と誤認させないため）。`BTreeSet` で重複除去と
/// 安定ソートを行う。
pub fn get_changed_files_since(path: &Path, since: &str) -> ChangedFiles {
    use std::collections::BTreeSet;
    let mut files: BTreeSet<String> = BTreeSet::new();

    let ok_diff = run_git_lines(path, &["diff", "--name-only", since], &mut files);
    let ok_untracked = run_git_lines(
        path,
        &["ls-files", "--others", "--exclude-standard"],
        &mut files,
    );

    if ok_diff && ok_untracked {
        ChangedFiles::Computed(files.into_iter().collect())
    } else {
        ChangedFiles::Unavailable
    }
}

/// `git <args>` を実行し、非空行を `out` に追加する。spawn 失敗・非ゼロ
/// 終了はいずれも `false` を返す（呼び出し側が算出不能として扱う）。
fn run_git_lines(path: &Path, args: &[&str], out: &mut std::collections::BTreeSet<String>) -> bool {
    match Command::new("git").args(args).current_dir(path).output() {
        Ok(o) if o.status.success() => {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let t = line.trim();
                if !t.is_empty() {
                    out.insert(t.to_string());
                }
            }
            true
        }
        _ => false,
    }
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
