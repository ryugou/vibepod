# VibePod v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust CLI tool (`vibepod` / `vp`) that wraps Docker to safely run AI coding agents with `--dangerously-skip-permissions` inside containers.

**Architecture:** Two commands (`init` + `run`) backed by a Docker API layer (`bollard`), config management (`serde`), and interactive UI (`dialoguer` / `indicatif`). The Docker runtime module is designed for reuse by v2's dashboard.

**Tech Stack:** Rust, bollard (Docker API), clap (CLI), dialoguer/indicatif (UI), tokio (async), serde (config)

**Spec:** `docs/superpowers/specs/2026-03-22-vibepod-design.md`

---

## File Structure

```
vibepod/
├── Cargo.toml
├── src/
│   ├── main.rs                # エントリポイント、tokio runtime 起動
│   ├── cli/
│   │   ├── mod.rs             # clap App 定義、サブコマンド登録
│   │   ├── init.rs            # `vibepod init` コマンドハンドラ
│   │   └── run.rs             # `vibepod run` コマンドハンドラ
│   ├── runtime/
│   │   ├── mod.rs             # re-export
│   │   └── docker.rs          # Docker API 操作（イメージビルド、コンテナ作成/起動/停止/削除、ログストリーム）
│   ├── config/
│   │   ├── mod.rs             # re-export
│   │   ├── global.rs          # ~/.vibepod/config.json の読み書き
│   │   └── projects.rs        # ~/.vibepod/projects.json の読み書き
│   └── ui/
│       ├── mod.rs             # re-export
│       ├── banner.rs          # AA バナー表示
│       └── prompts.rs         # 対話プロンプト（Agent 選択、プロジェクト登録確認）
├── templates/
│   └── Dockerfile             # バンドルする Dockerfile テンプレート
└── tests/
    ├── config_test.rs         # config 読み書きのユニットテスト
    ├── docker_test.rs         # Docker API 操作のインテグレーションテスト
    └── cli_test.rs            # CLI 引数パースのテスト
```

---

### Task 1: プロジェクト初期化 + Cargo.toml

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `templates/Dockerfile`

- [ ] **Step 1: Rust プロジェクトを初期化**

```bash
cd /Users/ryugo/Developer/src/settings/claude-devcontainer
cargo init --name vibepod
```

- [ ] **Step 2: Cargo.toml に依存クレートを追加**

`Cargo.toml` を以下の内容にする:

```toml
[package]
name = "vibepod"
version = "0.1.0"
edition = "2021"
description = "Safely run AI coding agents in Docker containers"
license = "MIT"
repository = "https://github.com/ryugou/vibepod"

[[bin]]
name = "vibepod"
path = "src/main.rs"

[dependencies]
bollard = "0.18"
clap = { version = "4", features = ["derive"] }
dialoguer = "0.11"
indicatif = "0.17"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dirs = "6"
rand = "0.8"
ctrlc = "3"
anyhow = "1"
futures-util = "0.3"
tar = "0.4"
chrono = { version = "0.4", features = ["serde"] }
libc = "0.2"
```

- [ ] **Step 3: 最小限の main.rs を作成**

```rust
use anyhow::Result;

fn main() -> Result<()> {
    println!("VibePod v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
```

- [ ] **Step 4: templates/Dockerfile を作成**

spec の Dockerfile をそのまま `templates/Dockerfile` に配置する。

- [ ] **Step 5: ビルドの確認**

Run: `cargo build`
Expected: コンパイル成功

- [ ] **Step 6: コミット**

```bash
git add Cargo.toml Cargo.lock src/main.rs templates/Dockerfile
git commit -m "feat: initialize vibepod rust project with dependencies"
```

---

### Task 2: Config モジュール（グローバル設定 + プロジェクト登録）

**Files:**
- Create: `src/config/mod.rs`
- Create: `src/config/global.rs`
- Create: `src/config/projects.rs`
- Create: `tests/config_test.rs`

- [ ] **Step 1: グローバル設定のテストを書く**

`tests/config_test.rs`:

```rust
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_save_and_load_global_config() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().to_path_buf();

    let config = vibepod::config::GlobalConfig {
        default_agent: "claude".to_string(),
        image: "vibepod-claude:latest".to_string(),
        claude_version: "latest".to_string(),
    };

    vibepod::config::save_global_config(&config, &config_dir).unwrap();
    let loaded = vibepod::config::load_global_config(&config_dir).unwrap();

    assert_eq!(loaded.default_agent, "claude");
    assert_eq!(loaded.image, "vibepod-claude:latest");
    assert_eq!(loaded.claude_version, "latest");
}

#[test]
fn test_load_global_config_not_found() {
    let tmp = TempDir::new().unwrap();
    let result = vibepod::config::load_global_config(&tmp.path().to_path_buf());
    assert!(result.is_err());
}
```

- [ ] **Step 2: tempfile を dev-dependencies に追加**

`Cargo.toml` に追加:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: テストが失敗することを確認**

Run: `cargo test --test config_test`
Expected: FAIL (モジュールが存在しない)

- [ ] **Step 4: config モジュールを実装**

`src/config/mod.rs`:

```rust
mod global;
mod projects;

pub use global::*;
pub use projects::*;
```

`src/config/global.rs`:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub default_agent: String,
    pub image: String,
    pub claude_version: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            default_agent: "claude".to_string(),
            image: "vibepod-claude:latest".to_string(),
            claude_version: "latest".to_string(),
        }
    }
}

pub fn save_global_config(config: &GlobalConfig, config_dir: &Path) -> Result<()> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("Failed to create config dir: {}", config_dir.display()))?;
    let path = config_dir.join("config.json");
    let json = serde_json::to_string_pretty(config)?;
    fs::write(&path, json)
        .with_context(|| format!("Failed to write config: {}", path.display()))?;
    Ok(())
}

pub fn load_global_config(config_dir: &Path) -> Result<GlobalConfig> {
    let path = config_dir.join("config.json");
    let json = fs::read_to_string(&path)
        .with_context(|| format!("Config not found: {}. Run `vibepod init` first.", path.display()))?;
    let config: GlobalConfig = serde_json::from_str(&json)?;
    Ok(config)
}

pub fn default_config_dir() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".vibepod"))
}
```

`src/config/projects.rs`:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub name: String,
    pub path: String,
    pub remote: Option<String>,
    pub registered_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectsConfig {
    pub projects: Vec<ProjectEntry>,
}

pub fn load_projects(config_dir: &Path) -> Result<ProjectsConfig> {
    let path = config_dir.join("projects.json");
    if !path.exists() {
        return Ok(ProjectsConfig::default());
    }
    let json = fs::read_to_string(&path)?;
    let config: ProjectsConfig = serde_json::from_str(&json)?;
    Ok(config)
}

pub fn save_projects(config: &ProjectsConfig, config_dir: &Path) -> Result<()> {
    fs::create_dir_all(config_dir)?;
    let path = config_dir.join("projects.json");
    let json = serde_json::to_string_pretty(config)?;
    fs::write(&path, json)?;
    Ok(())
}

pub fn is_project_registered(config: &ProjectsConfig, project_path: &str) -> bool {
    config.projects.iter().any(|p| p.path == project_path)
}

pub fn register_project(config: &mut ProjectsConfig, entry: ProjectEntry) {
    if !is_project_registered(config, &entry.path) {
        config.projects.push(entry);
    }
}
```

- [ ] **Step 5: main.rs に config モジュールを追加**

`src/main.rs` を更新:

```rust
pub mod config;

use anyhow::Result;

fn main() -> Result<()> {
    println!("VibePod v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
```

- [ ] **Step 6: テストが通ることを確認**

Run: `cargo test --test config_test`
Expected: PASS (2 tests)

- [ ] **Step 7: プロジェクト登録のテストを追加して通す**

`tests/config_test.rs` に追加:

```rust
#[test]
fn test_register_and_check_project() {
    let mut config = vibepod::config::ProjectsConfig::default();
    assert!(!vibepod::config::is_project_registered(&config, "/path/to/project"));

    vibepod::config::register_project(&mut config, vibepod::config::ProjectEntry {
        name: "my-project".to_string(),
        path: "/path/to/project".to_string(),
        remote: Some("github.com/user/repo".to_string()),
        registered_at: "2026-03-22T10:00:00Z".to_string(),
    });

    assert!(vibepod::config::is_project_registered(&config, "/path/to/project"));
    assert!(!vibepod::config::is_project_registered(&config, "/other/path"));
}
```

Run: `cargo test --test config_test`
Expected: PASS (3 tests)

- [ ] **Step 8: コミット**

```bash
git add src/config/ tests/config_test.rs Cargo.toml
git commit -m "feat: add config module for global settings and project registration"
```

---

### Task 3: UI モジュール（バナー + プロンプト）

**Files:**
- Create: `src/ui/mod.rs`
- Create: `src/ui/banner.rs`
- Create: `src/ui/prompts.rs`

- [ ] **Step 1: UI モジュールを作成**

`src/ui/mod.rs`:

```rust
pub mod banner;
pub mod prompts;
```

`src/ui/banner.rs`:

```rust
pub fn print_banner() {
    let banner = r#"
 ██╗   ██╗██╗██████╗ ███████╗██████╗  ██████╗ ██████╗
 ██║   ██║██║██╔══██╗██╔════╝██╔══██╗██╔═══██╗██╔══██╗
 ██║   ██║██║██████╔╝█████╗  ██████╔╝██║   ██║██║  ██║
 ╚██╗ ██╔╝██║██╔══██╗██╔══╝  ██╔═══╝ ██║   ██║██║  ██║
  ╚████╔╝ ██║██████╔╝███████╗██║     ╚██████╔╝██████╔╝
   ╚═══╝  ╚═╝╚═════╝ ╚══════╝╚═╝      ╚═════╝ ╚═════╝
"#;
    println!("{banner}");
}
```

`src/ui/prompts.rs`:

```rust
use anyhow::Result;
use dialoguer::{Confirm, Select};

pub fn select_agent() -> Result<String> {
    let items = vec![
        "Claude Code",
        "Gemini CLI (coming soon)",
        "OpenAI Codex (coming soon)",
    ];

    let selection = Select::new()
        .with_prompt("Which AI coding agent do you use?")
        .items(&items)
        .default(0)
        .interact()?;

    match selection {
        0 => Ok("claude".to_string()),
        _ => {
            println!("This agent is not yet supported. Using Claude Code.");
            Ok("claude".to_string())
        }
    }
}

pub fn confirm_project_registration(project_name: &str) -> Result<bool> {
    let result = Confirm::new()
        .with_prompt(format!("First time running in '{}'. Register it?", project_name))
        .default(true)
        .interact()?;
    Ok(result)
}

pub fn handle_existing_container(container_name: &str) -> Result<ExistingContainerAction> {
    let items = vec![
        "Attach to existing container",
        "Stop and start new container",
    ];

    let selection = Select::new()
        .with_prompt(format!("Container '{}' is already running", container_name))
        .items(&items)
        .default(0)
        .interact()?;

    match selection {
        0 => Ok(ExistingContainerAction::Attach),
        _ => Ok(ExistingContainerAction::Replace),
    }
}

pub enum ExistingContainerAction {
    Attach,
    Replace,
}
```

- [ ] **Step 2: main.rs に ui モジュールを追加**

`src/main.rs` に `pub mod ui;` を追加。

- [ ] **Step 3: ビルド確認**

Run: `cargo build`
Expected: コンパイル成功

- [ ] **Step 4: コミット**

```bash
git add src/ui/
git commit -m "feat: add UI module with banner and interactive prompts"
```

---

### Task 4: Docker ランタイムモジュール

**Files:**
- Create: `src/runtime/mod.rs`
- Create: `src/runtime/docker.rs`
- Create: `tests/docker_test.rs`

- [ ] **Step 1: Docker モジュールのインテグレーションテストを書く**

`tests/docker_test.rs`:

```rust
use vibepod::runtime::DockerRuntime;

/// These tests require Docker to be running. Run with:
/// `cargo test --test docker_test -- --ignored`

#[tokio::test]
#[ignore]
async fn test_docker_connection() {
    let runtime = DockerRuntime::new().await;
    assert!(runtime.is_ok(), "Docker should be running for this test");
}

#[tokio::test]
#[ignore]
async fn test_docker_ping() {
    let runtime = DockerRuntime::new().await.unwrap();
    let result = runtime.ping().await;
    assert!(result.is_ok());
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test docker_test`
Expected: FAIL (モジュールが存在しない)

- [ ] **Step 3: Docker ランタイムモジュールを実装**

`src/runtime/mod.rs`:

```rust
mod docker;
pub use docker::*;
```

`src/runtime/docker.rs`:

```rust
use anyhow::{Context, Result};
use bollard::container::{
    Config, CreateContainerOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions, ListContainersOptions,
};
use bollard::image::BuildImageOptions;
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::path::Path;

pub struct DockerRuntime {
    docker: Docker,
}

pub struct ContainerConfig {
    pub image: String,
    pub container_name: String,
    pub workspace_path: String,
    pub claude_dir: String,
    pub claude_json: Option<String>,  // None if ~/.claude.json doesn't exist
    pub args: Vec<String>,
    pub env_vars: Vec<String>,
    pub network_disabled: bool,
}

impl DockerRuntime {
    pub async fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .context("Failed to connect to Docker. Is Docker Desktop or OrbStack running?")?;
        Ok(Self { docker })
    }

    pub async fn ping(&self) -> Result<()> {
        self.docker.ping().await
            .context("Docker is not responding")?;
        Ok(())
    }

    pub async fn build_image(
        &self,
        dockerfile_content: &str,
        image_name: &str,
        build_args: HashMap<String, String>,
    ) -> Result<()> {
        // Create a tar archive containing the Dockerfile
        let mut header = tar::Header::new_gnu();
        let dockerfile_bytes = dockerfile_content.as_bytes();
        header.set_path("Dockerfile")?;
        header.set_size(dockerfile_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        let mut tar_builder = tar::Builder::new(Vec::new());
        tar_builder.append(&header, dockerfile_bytes)?;
        let tar_data = tar_builder.into_inner()?;

        let options = BuildImageOptions {
            t: image_name,
            buildargs: build_args.iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            ..Default::default()
        };

        let mut stream = self.docker.build_image(options, None, Some(tar_data.into()));

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => {
                    if let Some(stream) = output.stream {
                        print!("{}", stream);
                    }
                    if let Some(error) = output.error {
                        anyhow::bail!("Build error: {}", error);
                    }
                }
                Err(e) => anyhow::bail!("Build failed: {}", e),
            }
        }

        Ok(())
    }

    pub async fn image_exists(&self, image_name: &str) -> Result<bool> {
        match self.docker.inspect_image(image_name).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Returns (container_id, container_name) if a running container with the given name prefix is found.
    pub async fn find_running_container(&self, name_prefix: &str) -> Result<Option<(String, String)>> {
        let options = ListContainersOptions::<String> {
            all: false,
            ..Default::default()
        };
        let containers = self.docker.list_containers(Some(options)).await?;
        for container in containers {
            if let Some(names) = &container.names {
                for name in names {
                    let clean_name = name.trim_start_matches('/').to_string();
                    if clean_name.starts_with(name_prefix) {
                        if let Some(id) = &container.id {
                            return Ok(Some((id.clone(), clean_name)));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    pub async fn create_and_start_container(&self, config: &ContainerConfig) -> Result<String> {
        let mut mounts = vec![
            Mount {
                target: Some("/workspace".to_string()),
                source: Some(config.workspace_path.clone()),
                typ: Some(MountTypeEnum::BIND),
                read_only: Some(false),
                ..Default::default()
            },
            Mount {
                target: Some("/home/node/.claude".to_string()),
                source: Some(config.claude_dir.clone()),
                typ: Some(MountTypeEnum::BIND),
                read_only: Some(false),
                ..Default::default()
            },
        ];

        // ~/.claude.json is optional — only mount if it exists on host
        if let Some(ref claude_json_path) = config.claude_json {
            mounts.push(Mount {
                target: Some("/home/node/.claude.json".to_string()),
                source: Some(claude_json_path.clone()),
                typ: Some(MountTypeEnum::BIND),
                read_only: Some(true),
                ..Default::default()
            });
        }

        let host_config = HostConfig {
            mounts: Some(mounts),
            network_mode: if config.network_disabled {
                Some("none".to_string())
            } else {
                None
            },
            ..Default::default()
        };

        let mut env = config.env_vars.clone();
        // Ensure proper terminal
        env.push("TERM=xterm-256color".to_string());

        let container_config = Config {
            image: Some(config.image.clone()),
            cmd: Some(config.args.iter().map(|s| s.as_str()).collect()),
            host_config: Some(host_config),
            env: Some(env.iter().map(|s| s.as_str()).collect()),
            tty: Some(true),
            open_stdin: Some(true),
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: &config.container_name,
            ..Default::default()
        };

        let response = self.docker.create_container(Some(options), container_config).await
            .context("Failed to create container")?;

        self.docker.start_container(&response.id, None::<StartContainerOptions<String>>).await
            .context("Failed to start container")?;

        Ok(response.id)
    }

    pub async fn stream_logs(&self, container_id: &str) -> Result<()> {
        let options = LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        let mut stream = self.docker.logs(container_id, Some(options));

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => print!("{}", output),
                Err(e) => {
                    eprintln!("Log stream error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    pub async fn stop_container(&self, container_id: &str, timeout_secs: i64) -> Result<()> {
        let options = StopContainerOptions { t: timeout_secs };
        self.docker.stop_container(container_id, Some(options)).await
            .context("Failed to stop container")?;
        Ok(())
    }

    pub async fn remove_container(&self, container_id: &str) -> Result<()> {
        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };
        self.docker.remove_container(container_id, Some(options)).await
            .context("Failed to remove container")?;
        Ok(())
    }
}
```

- [ ] **Step 4: main.rs に runtime モジュールを追加**

`src/main.rs` に `pub mod runtime;` を追加。

- [ ] **Step 5: テストが通ることを確認**

Run: `cargo test --test docker_test`
Expected: PASS (Docker が動いていれば)

- [ ] **Step 6: コミット**

```bash
git add src/runtime/ tests/docker_test.rs
git commit -m "feat: add Docker runtime module with bollard API"
```

---

### Task 5: CLI モジュール（clap 定義 + init コマンド）

**Files:**
- Create: `src/cli/mod.rs`
- Create: `src/cli/init.rs`
- Create: `tests/cli_test.rs`

- [ ] **Step 1: CLI パースのテストを書く**

`tests/cli_test.rs`:

```rust
use clap::Parser;
use vibepod::cli::Cli;

#[test]
fn test_parse_init_command() {
    let cli = Cli::parse_from(["vibepod", "init"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Init { .. }));
}

#[test]
fn test_parse_run_with_resume() {
    let cli = Cli::parse_from(["vibepod", "run", "--resume"]);
    if let vibepod::cli::Commands::Run { resume, .. } = cli.command {
        assert!(resume);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_prompt() {
    let cli = Cli::parse_from(["vibepod", "run", "--prompt", "build the app"]);
    if let vibepod::cli::Commands::Run { prompt, .. } = cli.command {
        assert_eq!(prompt, Some("build the app".to_string()));
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_env() {
    let cli = Cli::parse_from(["vibepod", "run", "--resume", "--env", "KEY=VALUE", "--env", "FOO=BAR"]);
    if let vibepod::cli::Commands::Run { env, .. } = cli.command {
        assert_eq!(env, vec!["KEY=VALUE", "FOO=BAR"]);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_init_with_claude_version() {
    let cli = Cli::parse_from(["vibepod", "init", "--claude-version", "1.2.3"]);
    if let vibepod::cli::Commands::Init { claude_version, .. } = cli.command {
        assert_eq!(claude_version, Some("1.2.3".to_string()));
    } else {
        panic!("Expected Init command");
    }
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --test cli_test`
Expected: FAIL

- [ ] **Step 3: CLI モジュールを実装**

`src/cli/mod.rs`:

```rust
pub mod init;
pub mod run;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "vibepod", about = "Safely run AI coding agents in Docker containers")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize VibePod (build Docker image)
    Init {
        /// Pin Claude Code to a specific version
        #[arg(long)]
        claude_version: Option<String>,
    },
    /// Run AI agent in a container
    Run {
        /// Resume previous session
        #[arg(long)]
        resume: bool,
        /// Initial prompt for the agent
        #[arg(long)]
        prompt: Option<String>,
        /// Disable network access in the container
        #[arg(long)]
        no_network: bool,
        /// Environment variables to pass (KEY=VALUE)
        #[arg(long, num_args = 1)]
        env: Vec<String>,
    },
}
```

`src/cli/init.rs` (run コマンドの実装は Task 6 で行う):

```rust
use anyhow::{Context, Result};
use std::collections::HashMap;

use crate::config::{self, GlobalConfig};
use crate::runtime::DockerRuntime;
use crate::ui::{banner, prompts};

pub async fn execute(claude_version: Option<String>) -> Result<()> {
    banner::print_banner();

    // 1. Check Docker
    let runtime = DockerRuntime::new().await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;
    runtime.ping().await?;

    // 2. Select agent
    let agent = prompts::select_agent()?;

    // 3. Build image
    let version = claude_version.unwrap_or_else(|| "latest".to_string());
    let image_name = format!("vibepod-{}:latest", agent);

    println!("\n  Building Docker image: {}...", image_name);

    let dockerfile = include_str!("../../templates/Dockerfile");

    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    let mut build_args = HashMap::new();
    build_args.insert("HOST_UID".to_string(), uid.to_string());
    build_args.insert("HOST_GID".to_string(), gid.to_string());
    build_args.insert("CLAUDE_VERSION".to_string(), version.clone());

    match runtime.build_image(dockerfile, &image_name, build_args).await {
        Ok(_) => {}
        Err(e) => {
            eprintln!("\n  ✗ Build failed: {}", e);
            eprintln!("    Check your network connection and try `vibepod init` again.");
            return Err(e);
        }
    }

    // 4. Save config
    let config_dir = config::default_config_dir()?;
    let config = GlobalConfig {
        default_agent: agent,
        image: image_name,
        claude_version: version,
    };
    config::save_global_config(&config, &config_dir)?;

    println!("\n  Done! Run `vibepod run` in any git repo to start.\n");

    Ok(())
}
```

- [ ] **Step 4: main.rs を CLI エントリポイントに更新**

`src/main.rs`:

```rust
pub mod cli;
pub mod config;
pub mod runtime;
pub mod ui;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { claude_version } => {
            cli::init::execute(claude_version).await?;
        }
        Commands::Run {
            resume,
            prompt,
            no_network,
            env,
        } => {
            cli::run::execute(resume, prompt, no_network, env).await?;
        }
    }

    Ok(())
}
```

- [ ] **Step 5: `src/cli/run.rs` にスタブを作成（コンパイル通すため）**

```rust
use anyhow::{bail, Result};

pub async fn execute(
    _resume: bool,
    _prompt: Option<String>,
    _no_network: bool,
    _env_vars: Vec<String>,
) -> Result<()> {
    bail!("Not yet implemented. See Task 6.");
}
```

- [ ] **Step 6: テストが通ることを確認**

Run: `cargo test --test cli_test`
Expected: PASS (5 tests)

- [ ] **Step 7: コミット**

```bash
git add src/cli/ src/main.rs tests/cli_test.rs
git commit -m "feat: add CLI module with clap definitions and init command"
```

---

### Task 6: run コマンド実装

**Files:**
- Modify: `src/cli/run.rs`

- [ ] **Step 1: run.rs を完全実装**

`src/cli/run.rs`:

```rust
use anyhow::{bail, Context, Result};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Notify;

use crate::config::{self, ProjectEntry};
use crate::runtime::{ContainerConfig, DockerRuntime};
use crate::ui::prompts;

pub async fn execute(
    resume: bool,
    prompt: Option<String>,
    no_network: bool,
    env_vars: Vec<String>,
) -> Result<()> {
    // Validate: need either --resume or --prompt
    if !resume && prompt.is_none() {
        bail!("Either --resume or --prompt is required.\n  \
               Use --resume to continue a previous session, or\n  \
               Use --prompt \"...\" to start with a specific instruction.");
    }

    // 1. Check git repo
    let cwd = std::env::current_dir()?;
    let git_check = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&cwd)
        .output();

    if git_check.is_err() || !git_check.unwrap().status.success() {
        bail!("Not a git repository. Run this command inside a git-initialized directory.");
    }

    let project_name = cwd
        .file_name()
        .context("Cannot determine project name")?
        .to_string_lossy()
        .to_string();

    // Get remote URL (optional)
    let remote = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&cwd)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });

    // Get branch
    let branch = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&cwd)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("\n  ┌  VibePod");
    println!("  │");
    println!("  ◇  Detected git repository: {}", project_name);
    if let Some(ref r) = remote {
        println!("  │  Remote: {}", r);
    }
    println!("  │  Branch: {}", branch);
    println!("  │");

    // 2. Load config
    let config_dir = config::default_config_dir()?;
    let global_config = config::load_global_config(&config_dir)?;

    // 3. Check Docker & image
    let runtime = DockerRuntime::new().await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;

    if !runtime.image_exists(&global_config.image).await? {
        bail!("Docker image '{}' not found. Run `vibepod init` first.", global_config.image);
    }

    // 4. Check for existing container
    let name_prefix = format!("vibepod-{}", project_name);
    if let Some((existing_id, existing_name)) = runtime.find_running_container(&name_prefix).await? {
        match prompts::handle_existing_container(&existing_name)? {
            prompts::ExistingContainerAction::Attach => {
                println!("  ◇  Attaching to {}...", existing_name);
                runtime.stream_logs(&existing_id).await?;
                return Ok(());
            }
            prompts::ExistingContainerAction::Replace => {
                runtime.stop_container(&existing_id, 10).await?;
                runtime.remove_container(&existing_id).await?;
            }
        }
    }

    // 5. Project registration
    let mut projects = config::load_projects(&config_dir)?;
    let cwd_str = cwd.to_string_lossy().to_string();
    if !config::is_project_registered(&projects, &cwd_str) {
        if prompts::confirm_project_registration(&project_name)? {
            config::register_project(&mut projects, ProjectEntry {
                name: project_name.clone(),
                path: cwd_str.clone(),
                remote: remote.clone(),
                registered_at: chrono::Utc::now().to_rfc3339(),
            });
            config::save_projects(&projects, &config_dir)?;
        }
    }

    // 6. Build container args
    let mut args = vec!["--dangerously-skip-permissions".to_string()];
    if resume {
        args.push("--resume".to_string());
    }
    if let Some(p) = &prompt {
        args.push("--prompt".to_string());
        args.push(p.clone());
    }

    // 7. Generate container name
    let short_hash: String = (0..6)
        .map(|_| format!("{:x}", rand::random::<u8>() % 16))
        .collect();
    let container_name = format!("vibepod-{}-{}", project_name, short_hash);

    // Resolve paths
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let claude_dir = home.join(".claude");
    let claude_json = home.join(".claude.json");

    if !claude_dir.exists() {
        bail!("~/.claude not found. Please run `claude` once to log in first.");
    }

    // ~/.claude.json が存在しない場合はマウントしない（オプショナル）
    let mount_claude_json = claude_json.exists();

    println!("  ◇  Starting container...");
    println!("  │  Agent: Claude Code");
    println!("  │  Mode: --dangerously-skip-permissions");
    println!("  │  Mount: {} → /workspace", cwd.display());
    println!("  │");

    let container_config = ContainerConfig {
        image: global_config.image,
        container_name: container_name.clone(),
        workspace_path: cwd_str,
        claude_dir: claude_dir.to_string_lossy().to_string(),
        claude_json: if mount_claude_json {
            Some(claude_json.to_string_lossy().to_string())
        } else {
            None
        },
        args,
        env_vars,
        network_disabled: no_network,
    };

    let container_id = runtime.create_and_start_container(&container_config).await?;

    println!("  ◇  Container started: {}", container_name);
    println!("  │  Press Ctrl+C to stop the container.");
    println!("  └\n");

    // 8. Signal handling
    let shutdown = Arc::new(Notify::new());
    let shutdown_clone = shutdown.clone();

    ctrlc::set_handler(move || {
        shutdown_clone.notify_one();
    })?;

    // Stream logs until shutdown
    tokio::select! {
        _ = runtime.stream_logs(&container_id) => {}
        _ = shutdown.notified() => {
            println!("\n  Stopping container...");
            runtime.stop_container(&container_id, 10).await.ok();
            runtime.remove_container(&container_id).await.ok();
            println!("  Container stopped and removed.");
        }
    }

    Ok(())
}
```

- [ ] **Step 2: ビルド確認**

> Note: `ContainerConfig.claude_json` は Task 4 で既に `Option<String>` として定義済み。
> `create_and_start_container` も `Some` の場合のみマウントする実装済み。

Run: `cargo build`
Expected: コンパイル成功

- [ ] **Step 3: コミット**

```bash
git add src/cli/run.rs
git commit -m "feat: implement run command with container lifecycle management"
```

---

### Task 7: 統合テスト

**Files:**
- Create: `tests/integration_test.rs`

- [ ] **Step 1: 統合テストを書く**

`tests/integration_test.rs`:

```rust
use std::process::Command;
use std::path::PathBuf;

/// Get the path to the built binary
fn vibepod_bin() -> PathBuf {
    // cargo test sets CARGO_BIN_EXE_vibepod when using [[bin]] in Cargo.toml
    PathBuf::from(env!("CARGO_BIN_EXE_vibepod"))
}

#[test]
fn test_vibepod_version() {
    let output = Command::new(vibepod_bin())
        .arg("--version")
        .output()
        .expect("Failed to run vibepod");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("vibepod"));
}

#[test]
fn test_run_without_resume_or_prompt_fails() {
    let output = Command::new(vibepod_bin())
        .arg("run")
        .output()
        .expect("Failed to run vibepod");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--resume") || stderr.contains("--prompt"),
        "Error should mention --resume or --prompt, got: {}",
        stderr
    );
}

#[test]
fn test_run_outside_git_repo_fails() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = Command::new(vibepod_bin())
        .args(["run", "--resume"])
        .current_dir(tmp.path())
        .output()
        .expect("Failed to run vibepod");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git") || stderr.contains("repository"),
        "Error should mention git repository, got: {}",
        stderr
    );
}
```

- [ ] **Step 2: テストが通ることを確認**

Run: `cargo test --test integration_test`
Expected: PASS (3 tests)

- [ ] **Step 3: コミット**

```bash
git add tests/integration_test.rs
git commit -m "test: add integration tests for CLI error cases"
```

---

### Task 8: vp エイリアス + ビルド最適化

**Files:**
- Modify: `src/main.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: argv[0] によるエイリアス対応を main.rs に追加**

`src/main.rs` の先頭に以下を追加（`main` 関数の前）:

```rust
/// Check if invoked as "vp" alias — functionally identical to "vibepod"
fn get_binary_name() -> String {
    std::env::args()
        .next()
        .and_then(|arg| {
            std::path::Path::new(&arg)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "vibepod".to_string())
}
```

Cli の parse 部分を更新:

```rust
let cli = Cli::parse();
// "vp" as alias works identically — no special handling needed,
// clap already parses args regardless of binary name.
```

- [ ] **Step 2: Cargo.toml にリリースプロファイルを追加**

```toml
[profile.release]
strip = true
lto = true
codegen-units = 1
```

- [ ] **Step 3: リリースビルド確認**

Run: `cargo build --release`
Expected: コンパイル成功、`target/release/vibepod` が生成される

- [ ] **Step 4: バイナリサイズ確認**

Run: `ls -lh target/release/vibepod`
Expected: 数MB程度のシングルバイナリ

- [ ] **Step 5: コミット**

```bash
git add src/main.rs Cargo.toml
git commit -m "feat: add vp alias support and release build optimization"
```

---

### Task 9: README + ライセンス + CI

**Files:**
- Create: `README.md`
- Create: `LICENSE`
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: README.md を作成**

簡潔な README（インストール方法、使い方、セキュリティモデルの概要）を作成。

- [ ] **Step 2: MIT LICENSE を作成**

- [ ] **Step 3: GitHub Actions CI を作成**

`.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test
      - run: cargo clippy -- -D warnings
      - run: cargo fmt --check

  build:
    strategy:
      matrix:
        include:
          - target: x86_64-apple-darwin
            os: macos-13
          - target: aarch64-apple-darwin
            os: macos-latest
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
            use_cross: true
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Install cross (for cross-compilation)
        if: matrix.use_cross
        run: cargo install cross
      - name: Build (native)
        if: "!matrix.use_cross"
        run: cargo build --release --target ${{ matrix.target }}
      - name: Build (cross)
        if: matrix.use_cross
        run: cross build --release --target ${{ matrix.target }}
```

- [ ] **Step 4: コミット**

```bash
git add README.md LICENSE .github/
git commit -m "docs: add README, LICENSE, and CI workflow"
```

---

### Task 10: 手動テスト + 最終確認

- [ ] **Step 1: `vibepod init` を実行**

Run: `cargo run -- init`
Expected: バナー表示 → Agent 選択 → Docker イメージビルド成功

- [ ] **Step 2: `vibepod run --resume` を実行**

git リポジトリ内で実行:
Run: `cargo run -- run --resume`
Expected: リポジトリ検出 → コンテナ起動 → Claude Code がセッションを引き継いで起動

- [ ] **Step 3: Ctrl+C で停止**

Expected: コンテナが graceful に停止・削除される

- [ ] **Step 4: 全テスト最終実行**

Run: `cargo test`
Expected: 全 PASS

- [ ] **Step 5: 最終コミット**

```bash
git add -A
git commit -m "feat: VibePod v1 complete"
```
