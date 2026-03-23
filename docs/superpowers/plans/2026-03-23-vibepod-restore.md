# vibepod restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `vibepod restore` コマンドを実装し、エージェントの作業を完全に元に戻しつつレポートを残す。v1.2.0 としてリリースする。

**Architecture:** `vibepod run` 開始時に HEAD を `.vibepod/sessions.json` に記録。`vibepod restore` でセッション選択 → レポート生成 → `git reset --hard` + `git clean -fd` で復元。git 操作は `src/git.rs` ヘルパーに集約し、`run.rs` と `restore.rs` で共用する。

**Tech Stack:** Rust (clap, anyhow, dialoguer, chrono, serde, rand), Git (std::process::Command)

---

### Task 1: git ヘルパーモジュールの作成

`run.rs` にある git 操作を共通モジュールに抽出する。restore でも同じ操作が必要になるため。

**Files:**
- Create: `src/git.rs`
- Modify: `src/lib.rs:1-4`
- Modify: `src/cli/run.rs:1-62`

- [ ] **Step 1: テストを書く**

`tests/git_test.rs` を作成:

```rust
use std::process::Command;
use tempfile::TempDir;

fn init_test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[test]
fn test_get_head_hash() {
    let dir = init_test_repo();
    let hash = vibepod::git::get_head_hash(dir.path()).unwrap();
    assert_eq!(hash.len(), 40);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_get_current_branch() {
    let dir = init_test_repo();
    let branch = vibepod::git::get_current_branch(dir.path()).unwrap();
    assert!(!branch.is_empty());
}

#[test]
fn test_is_git_repo() {
    let dir = init_test_repo();
    assert!(vibepod::git::is_git_repo(dir.path()));

    let non_git = TempDir::new().unwrap();
    assert!(!vibepod::git::is_git_repo(non_git.path()));
}

#[test]
fn test_get_remote_url_none() {
    let dir = init_test_repo();
    let remote = vibepod::git::get_remote_url(dir.path());
    assert!(remote.is_none());
}

#[test]
fn test_commit_exists() {
    let dir = init_test_repo();
    let hash = vibepod::git::get_head_hash(dir.path()).unwrap();
    assert!(vibepod::git::commit_exists(dir.path(), &hash));
    assert!(!vibepod::git::commit_exists(dir.path(), "0000000000000000000000000000000000000000"));
}

#[test]
fn test_is_ancestor() {
    let dir = init_test_repo();
    let first = vibepod::git::get_head_hash(dir.path()).unwrap();
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "second"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let second = vibepod::git::get_head_hash(dir.path()).unwrap();
    assert!(vibepod::git::is_ancestor(dir.path(), &first, &second));
    assert!(!vibepod::git::is_ancestor(dir.path(), &second, &first));
}

#[test]
fn test_has_uncommitted_changes_clean() {
    let dir = init_test_repo();
    assert!(!vibepod::git::has_uncommitted_changes(dir.path()));
}

#[test]
fn test_has_uncommitted_changes_dirty() {
    let dir = init_test_repo();
    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    assert!(vibepod::git::has_uncommitted_changes(dir.path()));
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test git_test`
Expected: コンパイルエラー（`vibepod::git` が存在しない）

- [ ] **Step 3: git.rs を実装**

`src/git.rs` を作成:

```rust
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
```

- [ ] **Step 4: lib.rs に git モジュールを追加**

`src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod git;
pub mod runtime;
pub mod ui;
```

- [ ] **Step 5: テスト実行**

Run: `cargo test --test git_test`
Expected: 全テスト PASS

- [ ] **Step 6: run.rs を git ヘルパーに移行**

`src/cli/run.rs` の先頭の git 操作を `crate::git` に置き換える:

```rust
use crate::git;

// 1. Check git repo (行 18-27 を置き換え)
let cwd = std::env::current_dir()?;
if !git::is_git_repo(&cwd) {
    bail!("Not a git repository. Run this command inside a git-initialized directory.");
}

// Get remote URL (行 36-47 を置き換え)
let remote = git::get_remote_url(&cwd);

// Get branch (行 50-62 を置き換え)
let branch = git::get_current_branch(&cwd).unwrap_or_else(|_| "unknown".to_string());
```

`use std::process::Command;` は env file の op 処理とインタラクティブモードの docker 実行でまだ使うので残す。

- [ ] **Step 7: 既存テストが通ることを確認**

Run: `cargo test`
Expected: 全テスト PASS

- [ ] **Step 8: コミット**

```bash
git add src/git.rs src/lib.rs src/cli/run.rs tests/git_test.rs
git commit -m "refactor: extract git helpers into shared module"
```

---

### Task 2: セッション記録機能

`vibepod run` 開始時に HEAD を `.vibepod/sessions.json` に記録する。

**Files:**
- Create: `src/session.rs`
- Modify: `src/lib.rs`
- Modify: `src/cli/run.rs`

- [ ] **Step 1: テストを書く**

`tests/session_test.rs`:

```rust
use tempfile::TempDir;
use vibepod::session::{Session, SessionStore};

#[test]
fn test_add_and_load_session() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    let session = Session {
        id: "20260323-120000-a3f2".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: false,
    };

    store.add(session.clone()).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].id, "20260323-120000-a3f2");
    assert_eq!(loaded.sessions[0].head_before, "abc1234");
}

#[test]
fn test_session_limit_100() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    for i in 0..105 {
        let session = Session {
            id: format!("session-{:04}", i),
            started_at: format!("2026-03-23T{:02}:00:00+09:00", i % 24),
            head_before: format!("{:040x}", i),
            branch: "main".to_string(),
            prompt: "interactive".to_string(),
            claude_session_path: None,
            restored: false,
        };
        store.add(session).unwrap();
    }

    let loaded = store.load().unwrap();
    assert_eq!(loaded.sessions.len(), 100);
    // 最新 100 件が残る（0-4 が削除される）
    assert_eq!(loaded.sessions[0].id, "session-0005");
}

#[test]
fn test_mark_restored() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    let session = Session {
        id: "test-session".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: false,
    };
    store.add(session).unwrap();

    store.mark_restored("test-session").unwrap();
    let loaded = store.load().unwrap();
    assert!(loaded.sessions[0].restored);
}

#[test]
fn test_mark_restored_since() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    for id in ["s1", "s2", "s3"] {
        let session = Session {
            id: id.to_string(),
            started_at: "2026-03-23T12:00:00+09:00".to_string(),
            head_before: "abc".to_string(),
            branch: "main".to_string(),
            prompt: "interactive".to_string(),
            claude_session_path: None,
            restored: false,
        };
        store.add(session).unwrap();
    }

    store.mark_restored_since("s2").unwrap();
    let loaded = store.load().unwrap();
    assert!(!loaded.sessions[0].restored); // s1: unchanged
    assert!(loaded.sessions[1].restored);  // s2: restored
    assert!(loaded.sessions[2].restored);  // s3: restored
}

#[test]
fn test_restorable_sessions() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    let s1 = Session {
        id: "s1".to_string(),
        started_at: "2026-03-23T10:00:00+09:00".to_string(),
        head_before: "aaa".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: true,
    };
    let s2 = Session {
        id: "s2".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "bbb".to_string(),
        branch: "main".to_string(),
        prompt: "--resume".to_string(),
        claude_session_path: None,
        restored: false,
    };
    store.add(s1).unwrap();
    store.add(s2).unwrap();

    let restorable = store.restorable_sessions().unwrap();
    assert_eq!(restorable.len(), 1);
    assert_eq!(restorable[0].id, "s2");
}

#[test]
fn test_generate_session_id_unique() {
    let id1 = vibepod::session::generate_session_id();
    let id2 = vibepod::session::generate_session_id();
    assert_ne!(id1, id2);
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test session_test`
Expected: コンパイルエラー

- [ ] **Step 3: session.rs を実装**

`src/session.rs`:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub started_at: String,
    pub head_before: String,
    pub branch: String,
    pub prompt: String,
    pub claude_session_path: Option<String>,
    pub restored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionsData {
    pub sessions: Vec<Session>,
}

const MAX_SESSIONS: usize = 100;

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn sessions_path(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }

    pub fn load(&self) -> Result<SessionsData> {
        let path = self.sessions_path();
        if !path.exists() {
            return Ok(SessionsData::default());
        }
        let json = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let data: SessionsData = serde_json::from_str(&json)
            .context("セッション履歴ファイルが破損しています")?;
        Ok(data)
    }

    pub fn save(&self, data: &SessionsData) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::create_dir_all(self.dir.join("reports"))?;
        let json = serde_json::to_string_pretty(data)?;
        fs::write(self.sessions_path(), json)?;
        Ok(())
    }

    pub fn add(&self, session: Session) -> Result<()> {
        let mut data = self.load()?;
        data.sessions.push(session);
        // 上限を超えたら古いものから削除
        if data.sessions.len() > MAX_SESSIONS {
            let remove_count = data.sessions.len() - MAX_SESSIONS;
            data.sessions.drain(..remove_count);
        }
        self.save(&data)
    }

    pub fn mark_restored(&self, session_id: &str) -> Result<()> {
        let mut data = self.load()?;
        if let Some(session) = data.sessions.iter_mut().find(|s| s.id == session_id) {
            session.restored = true;
        }
        self.save(&data)
    }

    /// 指定セッション以降の全セッションを restored: true にする
    pub fn mark_restored_since(&self, session_id: &str) -> Result<()> {
        let mut data = self.load()?;
        let mut found = false;
        for session in data.sessions.iter_mut() {
            if session.id == session_id {
                found = true;
            }
            if found {
                session.restored = true;
            }
        }
        self.save(&data)
    }

    pub fn restorable_sessions(&self) -> Result<Vec<Session>> {
        let data = self.load()?;
        Ok(data
            .sessions
            .into_iter()
            .filter(|s| !s.restored)
            .collect())
    }

    pub fn reports_dir(&self) -> PathBuf {
        self.dir.join("reports")
    }
}

pub fn generate_session_id() -> String {
    let now = chrono::Local::now();
    let suffix: String = (0..4)
        .map(|_| format!("{:x}", rand::random::<u8>() % 16))
        .collect();
    format!("{}-{}", now.format("%Y%m%d-%H%M%S"), suffix)
}
```

- [ ] **Step 4: lib.rs に session モジュールを追加**

```rust
pub mod cli;
pub mod config;
pub mod git;
pub mod runtime;
pub mod session;
pub mod ui;
```

- [ ] **Step 5: テスト実行**

Run: `cargo test --test session_test`
Expected: 全テスト PASS

- [ ] **Step 6: コミット**

```bash
git add src/session.rs src/lib.rs tests/session_test.rs
git commit -m "feat: add session recording module"
```

---

### Task 3: vibepod run にセッション記録を組み込む

**Files:**
- Modify: `src/cli/run.rs`

- [ ] **Step 1: run.rs にセッション記録を追加**

`src/cli/run.rs` の先頭に use を追加:

```rust
use crate::session::{self, SessionStore};
```

git repo チェックの直後（行 27 の後）、プロジェクト名取得の前に以下を追加:

```rust
    // Record session for restore
    let head_before = git::get_head_hash(&cwd)?;
    let current_branch = git::get_current_branch(&cwd).unwrap_or_else(|_| "unknown".to_string());

    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir.clone());

    // Ensure .vibepod/ is in .gitignore
    let gitignore_path = cwd.join(".gitignore");
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if !content.lines().any(|l| l.trim() == ".vibepod/" || l.trim() == ".vibepod") {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&gitignore_path)?;
            use std::io::Write;
            writeln!(file, "\n.vibepod/")?;
        }
    } else {
        std::fs::write(&gitignore_path, ".vibepod/\n")?;
    }

    let prompt_label = if interactive {
        "interactive".to_string()
    } else if resume {
        "--resume".to_string()
    } else {
        prompt.as_deref().unwrap_or("").to_string()
    };

    let session = session::Session {
        id: session::generate_session_id(),
        started_at: chrono::Local::now().to_rfc3339(),
        head_before,
        branch: current_branch.clone(),
        prompt: prompt_label,
        claude_session_path: None,
        restored: false,
    };
    store.add(session)?;
```

また、`branch` 変数を git ヘルパーから取得済みの `current_branch` を使うように変更（元の行 50-62 を削除）。

- [ ] **Step 2: テスト実行**

Run: `cargo test`
Expected: 全テスト PASS

- [ ] **Step 3: コミット**

```bash
git add src/cli/run.rs
git commit -m "feat: record session on vibepod run start"
```

---

### Task 4: レポート生成

**Files:**
- Create: `src/report.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: テストを書く**

`tests/report_test.rs`:

```rust
use vibepod::report::generate_report;
use vibepod::session::Session;

#[test]
fn test_generate_report() {
    let session = Session {
        id: "20260323-120000-a3f2".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: Some("~/.claude/projects/test/session.jsonl".to_string()),
        restored: false,
    };

    let report = generate_report(
        &session,
        "def5678",
        "def5678 feat: add login page\nccc4444 fix: update styles",
        "A\tsrc/pages/login.rs\nM\tsrc/main.rs",
        " src/pages/login.rs | 45 +++\n src/main.rs | 3 +-\n 2 files changed, 46 insertions(+), 2 deletions(-)",
    );

    assert!(report.contains("# VibePod Session Report"));
    assert!(report.contains("abc1234"));
    assert!(report.contains("def5678"));
    assert!(report.contains("main"));
    assert!(report.contains("interactive"));
    assert!(report.contains("feat: add login page"));
    assert!(report.contains("session.jsonl"));
}

#[test]
fn test_generate_report_no_session_log() {
    let session = Session {
        id: "test".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: false,
    };

    let report = generate_report(&session, "def5678", "def5678 commit", "M\tfile.rs", "1 file changed");
    assert!(report.contains("なし"));
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test report_test`
Expected: コンパイルエラー

- [ ] **Step 3: report.rs を実装**

`src/report.rs`:

```rust
use crate::session::Session;

pub fn generate_report(
    session: &Session,
    current_head: &str,
    commit_log: &str,
    changed_files: &str,
    diff_stat: &str,
) -> String {
    let session_log = session
        .claude_session_path
        .as_deref()
        .unwrap_or("なし");

    format!(
        r#"# VibePod Session Report

- **実行日時:** {}
- **ブランチ:** {}
- **モード:** {}
- **開始HEAD:** {}
- **終了HEAD:** {}
- **Claude セッションログ:** {}

## コミット一覧

{}

## 変更ファイル一覧

{}

## 変更統計

{}
"#,
        session.started_at,
        session.branch,
        session.prompt,
        session.head_before,
        current_head,
        session_log,
        commit_log,
        changed_files,
        diff_stat,
    )
}
```

- [ ] **Step 4: lib.rs に report モジュールを追加**

```rust
pub mod cli;
pub mod config;
pub mod git;
pub mod report;
pub mod runtime;
pub mod session;
pub mod ui;
```

- [ ] **Step 5: テスト実行**

Run: `cargo test --test report_test`
Expected: 全テスト PASS

- [ ] **Step 6: コミット**

```bash
git add src/report.rs src/lib.rs tests/report_test.rs
git commit -m "feat: add session report generator"
```

---

### Task 5: vibepod restore コマンドの実装

**Files:**
- Create: `src/cli/restore.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: CLI パーステストを追加**

`tests/cli_test.rs` に追加:

```rust
#[test]
fn test_parse_restore_command() {
    let cli = Cli::parse_from(["vibepod", "restore"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Restore {}));
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test cli_test`
Expected: コンパイルエラー（`Restore` バリアントが存在しない）

- [ ] **Step 3: mod.rs に Restore サブコマンドを追加**

`src/cli/mod.rs`:

```rust
pub mod init;
pub mod restore;
pub mod run;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "vibepod",
    about = "Safely run AI coding agents in Docker containers"
)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize VibePod (build Docker image)
    Init {},
    /// Run AI agent in a container
    Run {
        /// Resume previous session
        #[arg(long)]
        resume: bool,
        /// Initial prompt for the agent (fire-and-forget mode)
        #[arg(long)]
        prompt: Option<String>,
        /// Disable network access in the container
        #[arg(long)]
        no_network: bool,
        /// Environment variables to pass (KEY=VALUE)
        #[arg(long, num_args = 1)]
        env: Vec<String>,
        /// Environment file (supports op:// references via 1Password CLI)
        #[arg(long)]
        env_file: Option<String>,
    },
    /// Restore workspace to a previous session state
    Restore {},
}
```

- [ ] **Step 4: restore.rs を実装**

`src/cli/restore.rs`:

```rust
use anyhow::{bail, Result};
use dialoguer::{Confirm, Select};

use crate::git;
use crate::report;
use crate::session::SessionStore;

pub fn execute() -> Result<()> {
    let cwd = std::env::current_dir()?;

    // 1. git リポジトリチェック
    if !git::is_git_repo(&cwd) {
        bail!("git リポジトリ内で実行してください");
    }

    // 2. .vibepod/ が git にトラッキングされていないことを確認
    let tracking_check = std::process::Command::new("git")
        .args(["ls-files", ".vibepod"])
        .current_dir(&cwd)
        .output()?;
    if !tracking_check.stdout.is_empty() {
        bail!(".vibepod/ が git 管理下にあります。.gitignore に追加してください");
    }

    // 3. セッション読み込み
    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir);

    let restorable = store.restorable_sessions()?;
    if restorable.is_empty() {
        if store.load()?.sessions.is_empty() {
            bail!("セッション履歴がありません。`vibepod run` を実行してください");
        } else {
            bail!("復元可能なセッションがありません");
        }
    }

    // 4. 未コミット変更チェック
    if git::has_uncommitted_changes(&cwd) {
        bail!("未コミットの変更があります。先にコミットするか stash してください");
    }

    // 5. セッション選択
    println!("\n  ┌  VibePod Restore");
    println!("  │");

    let items: Vec<String> = restorable
        .iter()
        .rev()
        .map(|s| {
            format!(
                "{} ({}) {} - {}",
                s.started_at.get(..19).unwrap_or(&s.started_at),
                s.branch,
                &s.head_before[..7],
                s.prompt
            )
        })
        .collect();

    let selection = Select::new()
        .with_prompt("  ◆  どのセッションに戻しますか？")
        .items(&items)
        .default(0)
        .interact()?;

    // 逆順で表示しているので、インデックスを逆算
    let selected = &restorable[restorable.len() - 1 - selection];

    // 6. HEAD チェック
    let current_head = git::get_head_hash(&cwd)?;

    if current_head == selected.head_before {
        bail!("変更がありません。復元の必要はありません");
    }

    if !git::commit_exists(&cwd, &selected.head_before) {
        bail!(
            "コミット {} が見つかりません。手動で git 操作された可能性があります",
            selected.head_before.get(..7).unwrap_or(&selected.head_before)
        );
    }

    // 7. ブランチチェック
    let current_branch = git::get_current_branch(&cwd).unwrap_or_default();
    if current_branch != selected.branch {
        println!(
            "  ⚠  セッション時のブランチ（{}）と現在のブランチ（{}）が異なります。",
            selected.branch, current_branch
        );
        if !Confirm::new()
            .with_prompt("  続行しますか？")
            .default(false)
            .interact()?
        {
            println!("  中止しました。");
            return Ok(());
        }
    }

    // 8. 祖先チェック
    if !git::is_ancestor(&cwd, &selected.head_before, &current_head) {
        println!("  ⚠  セッション開始時点のコミットが現在のブランチ履歴上にありません。");
        if !Confirm::new()
            .with_prompt("  強制的に戻しますか？")
            .default(false)
            .interact()?
        {
            println!("  中止しました。");
            return Ok(());
        }
    }

    // 9. 削除対象ファイル表示 + 確認
    println!("  │");
    println!("  ⚠  このセッション以降の全ての変更が巻き戻されます。");

    let untracked = git::get_untracked_files(&cwd)?;
    if !untracked.is_empty() {
        println!("  │");
        println!("  │  以下の未追跡ファイルも削除されます:");
        for f in &untracked {
            println!("  │    {}", f);
        }
    }

    println!("  │");
    if !Confirm::new()
        .with_prompt("  ◆  続行しますか？")
        .default(false)
        .interact()?
    {
        println!("  中止しました。");
        return Ok(());
    }

    // 10. レポート生成
    let commit_log =
        git::get_commit_log(&cwd, &selected.head_before, &current_head)?;
    let changed_files =
        git::get_changed_files(&cwd, &selected.head_before, &current_head)?;
    let diff_stat =
        git::get_diff_stat(&cwd, &selected.head_before, &current_head)?;

    let report_content = report::generate_report(
        selected,
        &current_head,
        &commit_log,
        &changed_files,
        &diff_stat,
    );

    let report_filename = format!(
        "{}.md",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S")
    );
    let report_path = store.reports_dir().join(&report_filename);
    std::fs::create_dir_all(store.reports_dir())?;
    std::fs::write(&report_path, &report_content)?;

    println!("  │");
    println!("  ◇  レポートを保存しました: {}", report_path.display());

    // 11. リセット
    println!("  │");
    println!(
        "  ◇  git reset --hard {}",
        selected.head_before.get(..7).unwrap_or(&selected.head_before)
    );
    git::reset_hard(&cwd, &selected.head_before)?;

    println!("  │  git clean -fd");
    git::clean_fd(&cwd)?;

    // 12. 選択セッション以降の全セッションに restored マーク
    store.mark_restored_since(&selected.id)?;

    println!("  │");
    println!("  ◇  復元完了！");
    println!("  └\n");

    Ok(())
}
```

- [ ] **Step 5: main.rs にルーティング追加**

`src/main.rs`:

```rust
use anyhow::Result;
use clap::Parser;
use vibepod::cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init {} => {
            vibepod::cli::init::execute().await?;
        }
        Commands::Run {
            resume,
            prompt,
            no_network,
            env,
            env_file,
        } => {
            vibepod::cli::run::execute(resume, prompt, no_network, env, env_file).await?;
        }
        Commands::Restore {} => {
            vibepod::cli::restore::execute()?;
        }
    }

    Ok(())
}
```

- [ ] **Step 6: テスト実行**

Run: `cargo test`
Expected: 全テスト PASS（cli_test の新しい restore テスト含む）

- [ ] **Step 7: コミット**

```bash
git add src/cli/restore.rs src/cli/mod.rs src/main.rs tests/cli_test.rs
git commit -m "feat: add vibepod restore command"
```

---

### Task 6: バージョンアップとリリース準備

**Files:**
- Modify: `Cargo.toml:3` (version)
- Modify: `README.md`
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Cargo.toml のバージョンを 1.2.0 に変更**

`Cargo.toml` の `version = "1.1.1"` を `version = "1.2.0"` に変更。

- [ ] **Step 2: release.yml の cargo publish を安全にする**

`cargo publish` を「already exists」時にスキップするように変更（今後の事故防止）:

```yaml
      - name: Publish to crates.io
        run: cargo publish 2>&1 | tee /tmp/cargo-publish.log || { grep -q 'already exists' /tmp/cargo-publish.log && echo "Version already published, skipping" || exit 1; }
```

- [ ] **Step 3: README.md にロードマップ更新と restore の使い方を追加**

ロードマップを更新し、`vibepod restore` の使い方セクションを追加。

- [ ] **Step 4: 全テスト + lint**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: 全 PASS

- [ ] **Step 5: コミット**

```bash
git add Cargo.toml Cargo.lock README.md .github/workflows/release.yml
git commit -m "chore: bump version to v1.2.0"
```

- [ ] **Step 6: ユーザーテスト**

**ユーザーに以下の動作確認を依頼（TTY 操作を含むため）:**

1. `vibepod run` → `.vibepod/sessions.json` が作成されることを確認
2. コンテナ内でファイルを変更・コミット
3. `vibepod restore` → セッション選択 → レポート生成 → リセット完了
4. `.vibepod/reports/` にレポートが保存されていることを確認
5. `git log` で元の HEAD に戻っていることを確認

**ユーザー確認後にタグ打ち・リリース。**

- [ ] **Step 7: タグとリリース**

```bash
git tag v1.2.0
git push origin main v1.2.0
```

CI 完了後、`cargo install vibepod` と `brew upgrade vibepod` で動作確認。
