# vibepod auth Implementation Plan (ARCHIVED)

> **注意:** この実装計画は credentials マウント + ロック機構ベースの旧アーキテクチャに基づいています。
> 実際の実装は `claude setup-token` + 環境変数方式に変更されました。
> 正式な仕様は `docs/superpowers/specs/2026-03-23-vibepod-auth-design.md` を参照してください。

**Goal:** `vibepod login` / `vibepod logout` コマンドを追加し、コンテナ内の Claude Code が独立した OAuth セッションで認証できるようにする。

**Architecture:** `src/auth.rs` が認証セッション管理（保存・読み込み・ロック・有効期限チェック・一時コピー・書き戻し）を担当。`vibepod login` は `--network host` でコンテナを起動し、`claude /login` の出力から URL をキャプチャしてホスト側でブラウザを開く。`vibepod run` は共有セッションのコピーをコンテナにマウントする。

**Tech Stack:** Rust (clap, anyhow, serde, serde_json, chrono, dirs), Docker (std::process::Command), URL detection (regex)

**Dependencies:** `regex` クレートを追加、`tempfile` を runtime dependency に昇格

---

### Task 1: auth モジュールの作成

認証セッションの保存・読み込み・ロック・有効期限チェック・一時コピー・書き戻しを実装する。

**Files:**
- Create: `src/auth.rs`
- Create: `tests/auth_test.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml` (regex 追加)

- [ ] **Step 1: テストを書く**

`tests/auth_test.rs`:

```rust
use tempfile::TempDir;
use vibepod::auth::{AuthManager, Credentials};

fn make_valid_credentials() -> Credentials {
    Credentials {
        claude_ai_oauth: serde_json::json!({
            "accessToken": "test-token",
            "refreshToken": "test-refresh",
            "expiresAt": (chrono::Utc::now().timestamp_millis() + 3600000) // 1 hour from now
        }),
    }
}

fn make_expired_credentials() -> Credentials {
    Credentials {
        claude_ai_oauth: serde_json::json!({
            "accessToken": "test-token",
            "refreshToken": "test-refresh",
            "expiresAt": (chrono::Utc::now().timestamp_millis() - 3600000) // 1 hour ago
        }),
    }
}

#[test]
fn test_save_and_load_shared_credentials() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();
    let loaded = manager.load_shared().unwrap().unwrap();
    assert_eq!(loaded.claude_ai_oauth["accessToken"], "test-token");
}

#[test]
fn test_load_shared_not_exists() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));
    assert!(manager.load_shared().unwrap().is_none());
}

#[test]
fn test_credentials_not_expired() {
    let creds = make_valid_credentials();
    assert!(!creds.is_expired());
}

#[test]
fn test_credentials_expired() {
    let creds = make_expired_credentials();
    assert!(creds.is_expired());
}

#[test]
fn test_save_and_load_isolated_credentials() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_isolated("my-container", &creds).unwrap();
    let loaded = manager.load_isolated("my-container").unwrap().unwrap();
    assert_eq!(loaded.claude_ai_oauth["accessToken"], "test-token");
}

#[test]
fn test_lock_acquire_and_release() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    assert!(manager.try_acquire_lock("container-1").unwrap());
    assert!(!manager.try_acquire_lock("container-2").unwrap());
    manager.release_lock().unwrap();
    assert!(manager.try_acquire_lock("container-2").unwrap());
}

#[test]
fn test_lock_info() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    manager.try_acquire_lock("my-container").unwrap();
    let info = manager.lock_info().unwrap();
    assert_eq!(info, Some("my-container".to_string()));
}

#[test]
fn test_copy_to_temp_and_writeback() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();

    let temp_path = manager.copy_shared_to_temp().unwrap();
    assert!(temp_path.exists());

    // Simulate token refresh by modifying temp file
    let mut updated = creds.clone();
    updated.claude_ai_oauth = serde_json::json!({
        "accessToken": "refreshed-token",
        "refreshToken": "test-refresh",
        "expiresAt": (chrono::Utc::now().timestamp_millis() + 7200000)
    });
    let json = serde_json::to_string_pretty(&updated).unwrap();
    std::fs::write(&temp_path, &json).unwrap();

    manager.writeback_shared(&temp_path).unwrap();

    let reloaded = manager.load_shared().unwrap().unwrap();
    assert_eq!(reloaded.claude_ai_oauth["accessToken"], "refreshed-token");
}

#[test]
fn test_delete_shared() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();
    manager.try_acquire_lock("test").unwrap();

    manager.delete_shared().unwrap();
    assert!(manager.load_shared().unwrap().is_none());
    assert!(manager.lock_info().unwrap().is_none());
}

#[test]
fn test_delete_all_isolated() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_isolated("c1", &creds).unwrap();
    manager.save_isolated("c2", &creds).unwrap();

    manager.delete_all_isolated().unwrap();
    assert!(manager.load_isolated("c1").unwrap().is_none());
    assert!(manager.load_isolated("c2").unwrap().is_none());
}

#[test]
fn test_url_detection() {
    assert!(vibepod::auth::detect_oauth_url("Visit https://platform.claude.com/oauth/authorize?foo=bar to log in").is_some());
    assert!(vibepod::auth::detect_oauth_url("No URL here").is_none());
    assert!(vibepod::auth::detect_oauth_url("Go to https://claude.ai/oauth/authorize?x=1").is_some());
}

#[cfg(unix)]
#[test]
fn test_file_permissions_600() {
    use std::os::unix::fs::MetadataExt;
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();

    let path = dir.path().join("config").join("auth").join("credentials.json");
    let mode = std::fs::metadata(&path).unwrap().mode();
    assert_eq!(mode & 0o777, 0o600);
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test auth_test`
Expected: コンパイルエラー

- [ ] **Step 3: Cargo.toml に regex を追加、tempfile を昇格**

`Cargo.toml` の `[dependencies]` に追加:

```toml
regex = "1"
tempfile = "3"
```

`[dev-dependencies]` から `tempfile` を削除。

- [ ] **Step 4: auth.rs を実装**

`src/auth.rs`:

```rust
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: serde_json::Value,
}

impl Credentials {
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.claude_ai_oauth.get("expiresAt") {
            if let Some(ts) = expires_at.as_i64() {
                let now = chrono::Utc::now().timestamp_millis();
                return ts < now;
            }
        }
        // If we can't determine expiry, assume not expired
        false
    }
}

pub struct AuthManager {
    config_dir: PathBuf,
    project_dir: PathBuf,
}

impl AuthManager {
    pub fn new(config_dir: PathBuf, project_dir: PathBuf) -> Self {
        Self {
            config_dir,
            project_dir,
        }
    }

    fn shared_dir(&self) -> PathBuf {
        self.config_dir.join("auth")
    }

    fn shared_path(&self) -> PathBuf {
        self.shared_dir().join("credentials.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.shared_dir().join("credentials.lock")
    }

    fn isolated_dir(&self) -> PathBuf {
        self.project_dir.join(".vibepod").join("auth").join("containers")
    }

    fn isolated_path(&self, name: &str) -> PathBuf {
        self.isolated_dir().join(format!("{}.json", name))
    }

    pub fn save_shared(&self, creds: &Credentials) -> Result<()> {
        let dir = self.shared_dir();
        fs::create_dir_all(&dir)?;
        let path = self.shared_path();
        let json = serde_json::to_string_pretty(creds)?;
        fs::write(&path, &json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn load_shared(&self) -> Result<Option<Credentials>> {
        let path = self.shared_path();
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let creds: Credentials = serde_json::from_str(&json)?;
        Ok(Some(creds))
    }

    pub fn save_isolated(&self, name: &str, creds: &Credentials) -> Result<()> {
        let dir = self.isolated_dir();
        fs::create_dir_all(&dir)?;
        let path = self.isolated_path(name);
        let json = serde_json::to_string_pretty(creds)?;
        fs::write(&path, &json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn load_isolated(&self, name: &str) -> Result<Option<Credentials>> {
        let path = self.isolated_path(name);
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path)?;
        let creds: Credentials = serde_json::from_str(&json)?;
        Ok(Some(creds))
    }

    pub fn try_acquire_lock(&self, container_name: &str) -> Result<bool> {
        let lock_path = self.lock_path();
        if lock_path.exists() {
            // Check if stale
            if let Ok(existing) = fs::read_to_string(&lock_path) {
                let existing_container = existing.trim();
                if !existing_container.is_empty() && is_container_running(existing_container) {
                    return Ok(false);
                }
            }
            // Stale lock, remove it
            fs::remove_file(&lock_path).ok();
        }
        fs::create_dir_all(self.shared_dir())?;
        fs::write(&lock_path, container_name)?;
        Ok(true)
    }

    pub fn release_lock(&self) -> Result<()> {
        let lock_path = self.lock_path();
        if lock_path.exists() {
            fs::remove_file(&lock_path)?;
        }
        Ok(())
    }

    pub fn lock_info(&self) -> Result<Option<String>> {
        let lock_path = self.lock_path();
        if !lock_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&lock_path)?;
        let name = content.trim().to_string();
        if name.is_empty() {
            Ok(None)
        } else {
            Ok(Some(name))
        }
    }

    pub fn copy_shared_to_temp(&self) -> Result<PathBuf> {
        let creds = self
            .load_shared()?
            .context("No shared credentials found")?;
        self.write_creds_to_temp(&creds)
    }

    pub fn copy_isolated_to_temp(&self, name: &str) -> Result<PathBuf> {
        let creds = self
            .load_isolated(name)?
            .context("No isolated credentials found")?;
        self.write_creds_to_temp(&creds)
    }

    fn write_creds_to_temp(&self, creds: &Credentials) -> Result<PathBuf> {
        use std::io::Write;
        let mut temp_file = tempfile::Builder::new()
            .prefix("vibepod-creds-")
            .suffix(".json")
            .tempfile()
            .context("Failed to create temp file")?;
        let json = serde_json::to_string_pretty(creds)?;
        temp_file.write_all(json.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(temp_file.path(), fs::Permissions::from_mode(0o600))?;
        }
        // Persist so it survives the NamedTempFile drop
        let path = temp_file.into_temp_path().keep()?;
        Ok(path)
    }

    pub fn writeback_shared(&self, temp_path: &Path) -> Result<()> {
        let json = fs::read_to_string(temp_path)?;
        let creds: Credentials = serde_json::from_str(&json)?;
        self.save_shared(&creds)
    }

    pub fn writeback_isolated(&self, name: &str, temp_path: &Path) -> Result<()> {
        let json = fs::read_to_string(temp_path)?;
        let creds: Credentials = serde_json::from_str(&json)?;
        self.save_isolated(name, &creds)
    }

    pub fn delete_shared(&self) -> Result<()> {
        let path = self.shared_path();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        self.release_lock()
    }

    pub fn delete_all_isolated(&self) -> Result<()> {
        let dir = self.isolated_dir();
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    pub fn cleanup_temp(&self, temp_path: &Path) {
        fs::remove_file(temp_path).ok();
    }
}

fn is_container_running(container_name: &str) -> bool {
    std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", container_name])
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).trim() == "true"
        })
        .unwrap_or(false)
}

pub fn detect_oauth_url(text: &str) -> Option<String> {
    let re = Regex::new(r"https://[a-zA-Z0-9._-]*claude[a-zA-Z0-9._-]*/oauth/authorize[^\s)\"'>]*")
        .unwrap();
    re.find(text).map(|m| m.as_str().to_string())
}

pub fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}
```

- [ ] **Step 5: lib.rs に auth モジュールを追加**

```rust
pub mod auth;
pub mod cli;
pub mod config;
pub mod git;
pub mod report;
pub mod runtime;
pub mod session;
pub mod ui;
```

- [ ] **Step 6: テスト実行**

Run: `cargo test --test auth_test`
Expected: 全テスト PASS

- [ ] **Step 7: コミット**

```bash
git add src/auth.rs src/lib.rs tests/auth_test.rs Cargo.toml Cargo.lock
git commit -m "feat: add auth session management module"
```

---

### Task 2: vibepod login コマンドの実装

**Files:**
- Create: `src/cli/login.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: CLI パーステストを追加**

`tests/cli_test.rs` に追加:

```rust
#[test]
fn test_parse_login_command() {
    let cli = Cli::parse_from(["vibepod", "login"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Login {}));
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test cli_test`
Expected: コンパイルエラー

- [ ] **Step 3: mod.rs に Login サブコマンドを追加**

`src/cli/mod.rs` に `Login {}` と `Logout` を追加:

```rust
pub mod init;
pub mod login;
pub mod logout;
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
    /// Authenticate for container use
    Login {},
    /// Remove authentication session
    Logout {
        /// Also remove all isolated container sessions
        #[arg(long)]
        all: bool,
    },
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
        /// Use isolated auth session for this container
        #[arg(long)]
        isolated: bool,
        /// Name for isolated session (default: vibepod-<project>-isolated)
        #[arg(long)]
        name: Option<String>,
    },
    /// Restore workspace to a previous session state
    Restore {},
}
```

- [ ] **Step 4: auth.rs にログインフロー関数を追加**

`login.rs` と `run.rs` の両方から使う共通ログインフロー。`src/auth.rs` の末尾に追加:

```rust
/// コンテナ内で claude /login を実行し、credentials を取得する共通フロー。
/// `-i`（TTY なし）で起動し、stdout をパイプで監視して URL をキャプチャする。
pub fn run_login_flow(image: &str) -> Result<Credentials> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let container_name = format!("vibepod-login-{}", chrono::Utc::now().timestamp_millis());

    // -i (no TTY) so stdout can be piped for URL capture
    let mut child = Command::new("docker")
        .args([
            "run",
            "-i",
            "--network", "host",
            "--name", &container_name,
            image,
            "claude", "/login",
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to start login container")?;

    // Monitor stdout for OAuth URL and pass through
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(url) = detect_oauth_url(&line) {
                if !open_browser(&url) {
                    eprintln!("  │  以下の URL をブラウザで開いてください:");
                    eprintln!("  │  {}", url);
                }
            }
            println!("{}", line);
        }
    }

    let status = child.wait()?;

    // Extract credentials via docker cp
    let temp_dir = std::env::temp_dir().join("vibepod-login");
    fs::create_dir_all(&temp_dir)?;
    let temp_creds = temp_dir.join("credentials.json");

    let cp_result = Command::new("docker")
        .args([
            "cp",
            &format!("{}:/home/vibepod/.claude/.credentials.json", container_name),
            &temp_creds.to_string_lossy(),
        ])
        .output();

    // Remove container
    Command::new("docker")
        .args(["rm", "-f", &container_name])
        .output()
        .ok();

    match cp_result {
        Ok(output) if output.status.success() => {
            let json = fs::read_to_string(&temp_creds)?;
            let creds: Credentials = serde_json::from_str(&json)?;
            fs::remove_file(&temp_creds).ok();
            Ok(creds)
        }
        _ => {
            if !status.success() {
                anyhow::bail!("ログインに失敗しました");
            }
            anyhow::bail!("コンテナからクレデンシャルを取得できませんでした")
        }
    }
}
```

- [ ] **Step 5: login.rs を実装**

`src/cli/login.rs` — `auth::run_login_flow` を呼び出す薄いラッパー:

```rust
use anyhow::{bail, Context, Result};

use crate::auth::{self, AuthManager};
use crate::config;
use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    println!("\n  ┌  VibePod Login");
    println!("  │");

    let runtime = DockerRuntime::new()
        .await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;

    let config_dir = config::default_config_dir()?;
    let global_config = config::load_global_config(&config_dir)?;

    if !runtime.image_exists(&global_config.image).await? {
        bail!(
            "Docker image '{}' not found. Run `vibepod init` first.",
            global_config.image
        );
    }

    let cwd = std::env::current_dir()?;
    let auth_manager = AuthManager::new(config_dir.clone(), cwd);

    if let Some(existing) = auth_manager.load_shared()? {
        if !existing.is_expired() {
            println!("  ⚠  既存のセッションがあります。");
            if !dialoguer::Confirm::new()
                .with_prompt("  上書きしますか？")
                .default(false)
                .interact()?
            {
                println!("  └\n");
                return Ok(());
            }
        }
    }

    println!("  ◇  コンテナ用の認証セッションを作成します");
    println!("  │");

    let creds = auth::run_login_flow(&global_config.image)?;
    auth_manager.save_shared(&creds)?;

    println!("  │");
    println!("  ◇  認証完了！");
    println!(
        "  │  セッションを保存しました: ~/.config/vibepod/auth/credentials.json"
    );
    println!("  └\n");

    Ok(())
}
```

- [ ] **Step 6: main.rs にルーティング追加**

`src/main.rs` の match に追加:

```rust
Commands::Login {} => {
    vibepod::cli::login::execute().await?;
}
```

- [ ] **Step 7: テスト実行**

Run: `cargo test`
Expected: 全テスト PASS

- [ ] **Step 8: コミット**

```bash
git add src/cli/login.rs src/cli/mod.rs src/main.rs tests/cli_test.rs
git commit -m "feat: add vibepod login command"
```

---

### Task 3: vibepod logout コマンドの実装

**Files:**
- Create: `src/cli/logout.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: CLI パーステストを追加**

`tests/cli_test.rs` に追加:

```rust
#[test]
fn test_parse_logout_command() {
    let cli = Cli::parse_from(["vibepod", "logout"]);
    assert!(matches!(
        cli.command,
        vibepod::cli::Commands::Logout { all: false }
    ));
}

#[test]
fn test_parse_logout_all_command() {
    let cli = Cli::parse_from(["vibepod", "logout", "--all"]);
    if let vibepod::cli::Commands::Logout { all } = cli.command {
        assert!(all);
    } else {
        panic!("Expected Logout command");
    }
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test cli_test`
Expected: コンパイルエラー

- [ ] **Step 3: logout.rs を実装**

`src/cli/logout.rs`:

```rust
use anyhow::{Context, Result};

use crate::auth::AuthManager;
use crate::config;

pub fn execute(all: bool) -> Result<()> {
    let config_dir = config::default_config_dir()?;
    let cwd = std::env::current_dir()?;
    let auth_manager = AuthManager::new(config_dir, cwd);

    println!("\n  ┌  VibePod Logout");
    println!("  │");

    // Delete shared session
    auth_manager.delete_shared()?;
    println!("  ◇  共有セッションを削除しました");

    if all {
        auth_manager.delete_all_isolated()?;
        println!("  ◇  全てのコンテナ専用セッションを削除しました");
    }

    println!("  └\n");
    Ok(())
}
```

- [ ] **Step 4: main.rs にルーティング追加**

```rust
Commands::Logout { all } => {
    vibepod::cli::logout::execute(all)?;
}
```

- [ ] **Step 5: テスト実行**

Run: `cargo test`
Expected: 全テスト PASS

- [ ] **Step 6: コミット**

```bash
git add src/cli/logout.rs src/main.rs tests/cli_test.rs
git commit -m "feat: add vibepod logout command"
```

---

### Task 4: vibepod run の認証フロー組み込み

**Files:**
- Modify: `src/cli/run.rs`

- [ ] **Step 1: CLI パーステストを追加**

`tests/cli_test.rs` に追加:

```rust
#[test]
fn test_parse_run_with_isolated() {
    let cli = Cli::parse_from(["vibepod", "run", "--isolated"]);
    if let vibepod::cli::Commands::Run { isolated, name, .. } = cli.command {
        assert!(isolated);
        assert!(name.is_none());
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_isolated_name() {
    let cli = Cli::parse_from(["vibepod", "run", "--isolated", "--name", "my-session"]);
    if let vibepod::cli::Commands::Run { isolated, name, .. } = cli.command {
        assert!(isolated);
        assert_eq!(name, Some("my-session".to_string()));
    } else {
        panic!("Expected Run command");
    }
}
```

- [ ] **Step 2: run.rs の execute シグネチャを更新**

```rust
pub async fn execute(
    resume: bool,
    prompt: Option<String>,
    no_network: bool,
    env_vars: Vec<String>,
    env_file: Option<String>,
    isolated: bool,
    session_name: Option<String>,
) -> Result<()> {
```

- [ ] **Step 3: main.rs の Run マッチを更新**

```rust
Commands::Run {
    resume,
    prompt,
    no_network,
    env,
    env_file,
    isolated,
    name,
} => {
    vibepod::cli::run::execute(resume, prompt, no_network, env, env_file, isolated, name)
        .await?;
}
```

- [ ] **Step 4: run.rs の認証セクションを置き換え**

既存の credentials チェック（行 230-238）を新しい認証フローに置き換え:

```rust
    // 8. Auth: prepare credentials for container
    let config_dir_for_auth = config::default_config_dir()?;
    let auth_manager = crate::auth::AuthManager::new(config_dir_for_auth, cwd.clone());
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let claude_json = home.join(".claude.json");

    // Resolve isolated session name early (avoids ownership issues)
    let iso_name = if isolated {
        Some(session_name
            .clone()
            .unwrap_or_else(|| format!("vibepod-{}-isolated", project_name)))
    } else {
        None
    };

    let (temp_creds_path, is_shared_lock) = if isolated {
        let name = iso_name.as_ref().unwrap();

        match auth_manager.load_isolated(name)? {
            Some(creds) if !creds.is_expired() => {
                let temp = auth_manager.copy_isolated_to_temp(name)?;
                (temp, false)
            }
            _ => {
                // Need to login for isolated session
                println!("  ◇  コンテナ専用セッションが必要です。ログインしてください...");
                let creds = crate::auth::run_login_flow(&global_config.image)?;
                auth_manager.save_isolated(name, &creds)?;
                let temp = auth_manager.copy_isolated_to_temp(name)?;
                (temp, false)
            }
        }
    } else {
        // Shared mode
        match auth_manager.load_shared()? {
            None => {
                bail!("`vibepod login` を先に実行してください");
            }
            Some(creds) if creds.is_expired() => {
                bail!("セッションの有効期限が切れています。`vibepod login` を再実行してください");
            }
            Some(_) => {
                if !auth_manager.try_acquire_lock(&container_name)? {
                    let lock_info = auth_manager.lock_info()?.unwrap_or_default();
                    println!(
                        "  ⚠  共有セッションは別のコンテナ ({}) で使用中です。",
                        lock_info
                    );
                    bail!("`vibepod run --isolated` を使用してください");
                }
                let temp = auth_manager.copy_shared_to_temp()?;
                (temp, true)
            }
        }
    };
```

- [ ] **Step 5: credentials マウントを一時ファイルに変更**

インタラクティブモードの docker_args で:

```rust
        // Replace old credential mount with temp file mount
        "-v".to_string(),
        format!(
            "{}:/home/vibepod/.claude/.credentials.json",
            temp_creds_path.display()
        ),
```

fire-and-forget モードの ContainerConfig でも同様に `temp_creds_path` を使用。
また `src/runtime/docker.rs` の `ContainerConfig` に `credentials_readonly: bool` フィールドを追加し、`create_and_start_container` の credentials マウントで `read_only: Some(config.credentials_readonly)` を使うように変更。新しい認証フローでは `credentials_readonly: false` を渡す（コンテナ内でトークンリフレッシュが走るため）。

- [ ] **Step 6: コンテナ終了時の書き戻しとロック解放を追加**

コンテナ終了後（インタラクティブ/fire-and-forget 両方）に:

```rust
    // Writeback and cleanup (order: writeback THEN unlock)
    if is_shared_lock {
        auth_manager.writeback_shared(&temp_creds_path).ok();
        auth_manager.release_lock().ok();
    } else if let Some(ref name) = iso_name {
        auth_manager.writeback_isolated(name, &temp_creds_path).ok();
    }
    auth_manager.cleanup_temp(&temp_creds_path);
```

- [ ] **Step 7: テスト実行**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: 全テスト PASS

- [ ] **Step 8: コミット**

```bash
git add src/cli/run.rs src/cli/mod.rs src/main.rs tests/cli_test.rs
git commit -m "feat: integrate auth flow into vibepod run"
```

---

### Task 5: 全体テストとリリース準備

**Files:**
- Modify: `README.md`
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: 全テスト + lint**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: 全 PASS

- [ ] **Step 2: README.md に login/logout/--isolated の使い方を追加**

使い方セクションに認証コマンドの説明を追加。

- [ ] **Step 3: コミット**

```bash
git add README.md
git commit -m "docs: add login/logout/isolated auth docs to README"
```

- [ ] **Step 4: ユーザーテスト**

**ユーザーに以下の動作確認を依頼（TTY 操作を含むため）:**

1. `cargo run -- login` → ブラウザ認可 → セッション保存
2. `cargo run -- run` → 認証成功で Claude Code が使える
3. 別ターミナルで `cargo run -- run` → `--isolated` への案内表示
4. `cargo run -- run --isolated` → コンテナ専用ログイン → 動作確認
5. `cargo run -- logout` → セッション削除
6. `cargo run -- restore` → restore が引き続き動作することを確認

**ユーザー確認後にタグ打ち・リリース。**

- [ ] **Step 5: タグとリリース**

```bash
git tag v1.2.0
git push origin main v1.2.0
```
