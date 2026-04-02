use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::config::{self, ProjectEntry};
use crate::git;
use crate::runtime::{ContainerConfig, DockerRuntime};
use crate::session::{self, SessionStore};
use crate::ui::banner;
use crate::ui::prompts;

pub struct RunOptions {
    pub resume: bool,
    pub prompt: Option<String>,
    pub no_network: bool,
    pub env_vars: Vec<String>,
    pub env_file: Option<String>,
    pub bridge: bool,
    pub notify_delay: u64,
    pub slack_channel: Option<String>,
    pub llm_provider: String,
    pub lang: Option<String>,
    pub worktree: bool,
    pub review: bool,
}

struct RunContext {
    runtime: DockerRuntime,
    container_name: String,
    effective_workspace: String,
    claude_args: Vec<String>,
    resolved_env_vars: Vec<String>,
    setup_cmd: Option<String>,
    temp_claude_json: Option<std::path::PathBuf>,
    global_config: config::GlobalConfig,
    home: std::path::PathBuf,
    worktree_branch_name: Option<String>,
    worktree_dir_name: Option<String>,
    lang_display: String,
    session_id: String,
    project_name: String,
}

pub fn detect_languages(workspace: &std::path::Path) -> Vec<(String, &'static str)> {
    let mut langs = Vec::new();
    if workspace.join("Cargo.toml").exists() {
        langs.push(("rust".to_string(), "Cargo.toml"));
    }
    if workspace.join("package.json").exists() {
        langs.push(("node".to_string(), "package.json"));
    }
    if workspace.join("go.mod").exists() {
        langs.push(("go".to_string(), "go.mod"));
    }
    if workspace.join("pyproject.toml").exists() {
        langs.push(("python".to_string(), "pyproject.toml"));
    } else if workspace.join("requirements.txt").exists() {
        langs.push(("python".to_string(), "requirements.txt"));
    }
    if workspace.join("pom.xml").exists() {
        langs.push(("java".to_string(), "pom.xml"));
    } else if workspace.join("build.gradle").exists() {
        langs.push(("java".to_string(), "build.gradle"));
    } else if workspace.join("build.gradle.kts").exists() {
        langs.push(("java".to_string(), "build.gradle.kts"));
    }
    langs
}

pub fn get_lang_install_cmd(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some("apt-get update && apt-get install -y build-essential && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && . $HOME/.cargo/env"),
        "node" => Some("curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && apt-get install -y nodejs"),
        "python" => Some("apt-get update && apt-get install -y python3 python3-pip python3-venv"),
        "go" => Some("ARCH=$(uname -m) && GOARCH=$([ \"$ARCH\" = \"aarch64\" ] && echo arm64 || echo amd64) && curl -fsSL https://go.dev/dl/go1.24.2.linux-${GOARCH}.tar.gz | tar -C /usr/local -xzf - && export PATH=$PATH:/usr/local/go/bin"),
        "java" => Some("apt-get update && apt-get install -y default-jdk"),
        _ => None,
    }
}

pub fn validate_slack_channel_id(id: &str) -> bool {
    id.starts_with('C') && id.len() >= 9
}

pub fn build_review_prompt(prompt: &str, review: bool) -> String {
    if review {
        format!(
            "{}\n\n---\n\n\
            実装が完了したら、以下のレビューフローを実行すること:\n\
            1. 現在のブランチが main の場合は、`git checkout -b <適切なブランチ名>` で新しいフィーチャーブランチを作成する。main 以外のブランチにいる場合はそのまま使う\n\
            2. 変更内容をコミットする（Conventional Commits 準拠）\n\
            3. `git push -u origin <ブランチ名>` でリモートに push する\n\
            4. `gh pr create` で PR を作成する（ベースブランチは main）\n\
            5. `gh pr edit <PR番号> --add-reviewer copilot` で GitHub Copilot のレビューを依頼する\n\
            6. Copilot のレビューが届くまで 30 秒間隔で最大 10 回 `gh api repos/{{owner}}/{{repo}}/pulls/{{number}}/reviews` を実行して確認する。\
               `gh pr review` や `gh pr comment` などの書き込み系コマンドは絶対に使わないこと（意図しないレビューコメントが作成されるため）\n\
            7. Copilot のレビューコメントがあれば内容を読み、指摘された問題を修正する\n\
            8. 修正をコミットして `git push` で PR を更新する\n\
            9. 最終的な PR の URL を出力する",
            prompt
        )
    } else {
        prompt.to_string()
    }
}

fn build_container_config(ctx: &RunContext, image: String, no_network: bool) -> ContainerConfig {
    let gitconfig = ctx.home.join(".gitconfig");
    ContainerConfig {
        image,
        container_name: ctx.container_name.clone(),
        workspace_path: ctx.effective_workspace.clone(),
        claude_json: ctx
            .temp_claude_json
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        args: {
            let mut full = vec!["claude".to_string()];
            full.extend(ctx.claude_args.clone());
            full
        },
        gitconfig: if gitconfig.exists() {
            Some(gitconfig.to_string_lossy().to_string())
        } else {
            None
        },
        env_vars: ctx.resolved_env_vars.clone(),
        network_disabled: no_network,
        setup_cmd: ctx.setup_cmd.clone(),
    }
}

async fn prepare_context(opts: &RunOptions) -> Result<Option<RunContext>> {
    let interactive = !opts.resume && opts.prompt.is_none();

    // 1. Check git repo
    let cwd = std::env::current_dir()?;
    if !git::is_git_repo(&cwd) {
        bail!("Not a git repository. Run this command inside a git-initialized directory.");
    }
    let cwd_str = cwd.to_string_lossy().to_string();

    if opts.worktree && opts.prompt.is_none() {
        bail!("--worktree requires --prompt");
    }
    if opts.review && opts.prompt.is_none() {
        bail!("--review requires --prompt");
    }

    // Record session for restore
    let head_before = git::get_head_hash(&cwd)?;
    let current_branch = git::get_current_branch(&cwd).unwrap_or_else(|_| "unknown".to_string());

    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir.clone());

    // Ensure .worktrees/ exists for superpowers skill (must find it without asking the user)
    let worktrees_dir = cwd.join(".worktrees");
    if !worktrees_dir.exists() {
        std::fs::create_dir_all(&worktrees_dir)?;
    }

    // Ensure .vibepod/ and .worktrees/ are in .gitignore
    let gitignore_path = cwd.join(".gitignore");
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        let needs_vibepod = !content
            .lines()
            .any(|l| l.trim() == ".vibepod/" || l.trim() == ".vibepod");
        let needs_worktrees = !content
            .lines()
            .any(|l| l.trim() == ".worktrees/" || l.trim() == ".worktrees");
        if needs_vibepod || needs_worktrees {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&gitignore_path)?;
            use std::io::Write;
            if needs_vibepod {
                writeln!(file, "\n.vibepod/")?;
            }
            if needs_worktrees {
                writeln!(file, "\n.worktrees/")?;
            }
        }
    } else {
        std::fs::write(&gitignore_path, ".vibepod/\n.worktrees/\n")?;
    }

    let prompt_label = if interactive {
        "interactive".to_string()
    } else if opts.resume {
        "--resume".to_string()
    } else {
        opts.prompt.as_deref().unwrap_or("").to_string()
    };

    let session_id = session::generate_session_id();
    let session_record = session::Session {
        id: session_id.clone(),
        started_at: chrono::Local::now().to_rfc3339(),
        head_before,
        branch: current_branch.clone(),
        prompt: prompt_label,
        claude_session_path: None,
        restored: false,
    };
    store.add(session_record)?;

    let project_name = cwd
        .file_name()
        .context("Cannot determine project name")?
        .to_string_lossy()
        .to_string();

    // Get remote URL (optional)
    let remote = git::get_remote_url(&cwd);

    // Get branch
    let branch = current_branch;

    banner::print_banner();
    if opts.prompt.is_some() {
        println!();
        println!("Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("Remote: {}", r);
        }
        println!("Branch: {}", branch);
        println!();
    } else {
        println!("  ┌");
        println!("  │");
        println!("  ◇  Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("  │  Remote: {}", r);
        }
        println!("  │  Branch: {}", branch);
        println!("  │");
    }

    // Worktree creation
    let (effective_workspace, worktree_branch_name, worktree_dir_name) = if opts.worktree {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let branch_name = format!("vibepod/prompt-{}", ts);
        let dir_name = format!("vibepod-prompt-{}", ts);
        let wt_path = cwd.join(".worktrees").join(&dir_name);

        let wt_path_str = wt_path.to_string_lossy().to_string();
        let output = Command::new("git")
            .args(["worktree", "add", &wt_path_str, "-b", &branch_name])
            .current_dir(&cwd)
            .output()
            .context("Failed to run git worktree")?;

        if !output.status.success() {
            bail!(
                "Failed to create worktree: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        println!("Created worktree: .worktrees/{}", dir_name);
        println!("Branch: {}", branch_name);

        (
            wt_path.to_string_lossy().to_string(),
            Some(branch_name),
            Some(dir_name),
        )
    } else {
        (cwd_str.clone(), None, None)
    };

    // Language detection
    let detected_langs: Vec<(String, &'static str)> = if opts.lang.is_none() {
        detect_languages(&cwd)
    } else {
        Vec::new()
    };

    let (lang_names, lang_display): (Vec<String>, String) = if let Some(ref l) = opts.lang {
        (vec![l.clone()], format!("{} (--lang)", l))
    } else if detected_langs.len() == 1 {
        let (name, file) = &detected_langs[0];
        (
            vec![name.clone()],
            format!("{} (detected from {})", name, file),
        )
    } else if detected_langs.len() > 1 {
        let names: Vec<String> = detected_langs.iter().map(|(n, _)| n.clone()).collect();
        let display = format!("{} (auto-detected)", names.join(", "));
        (names, display)
    } else {
        (Vec::new(), String::new())
    };

    let setup_cmd: Option<String> = {
        let cmds: Vec<&str> = lang_names
            .iter()
            .filter_map(|l| get_lang_install_cmd(l))
            .collect();
        if cmds.is_empty() {
            None
        } else {
            Some(cmds.join(" && "))
        }
    };

    // 2. Load config
    let config_dir = config::default_config_dir()?;
    let global_config = config::load_global_config(&config_dir)?;

    // 3. Check Docker & image
    let runtime = DockerRuntime::new()
        .await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;

    if !runtime.image_exists(&global_config.image).await? {
        bail!(
            "Docker image '{}' not found. Run `vibepod init` first.",
            global_config.image
        );
    }

    // 4. Check for existing container
    let name_prefix = format!("vibepod-{}", project_name);
    if let Some((existing_id, existing_name)) = runtime.find_running_container(&name_prefix).await?
    {
        if opts.bridge {
            // bridge モードでは既存コンテナを置き換える（bridge は新規コンテナの attach が必要）
            println!("  ◇  Replacing existing container for bridge mode...");
            runtime.stop_container(&existing_id, 10).await?;
            runtime.remove_container(&existing_id).await?;
        } else {
            match prompts::handle_existing_container(&existing_name)? {
                prompts::ExistingContainerAction::Attach => {
                    println!("  ◇  Attaching to {}...", existing_name);
                    runtime.stream_logs(&existing_id).await?;
                    return Ok(None);
                }
                prompts::ExistingContainerAction::Replace => {
                    runtime.stop_container(&existing_id, 10).await?;
                    runtime.remove_container(&existing_id).await?;
                }
            }
        }
    }

    // 5. Project registration
    let mut projects = config::load_projects(&config_dir)?;
    if !config::is_project_registered(&projects, &cwd_str)
        && prompts::confirm_project_registration(&project_name)?
    {
        config::register_project(
            &mut projects,
            ProjectEntry {
                name: project_name.clone(),
                path: cwd_str.clone(),
                remote: remote.clone(),
                registered_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        config::save_projects(&projects, &config_dir)?;
    }

    // 6. Build claude args
    let mut claude_args: Vec<String> = Vec::new();
    if !interactive {
        claude_args.push("--dangerously-skip-permissions".to_string());
    }
    if opts.resume {
        claude_args.push("--resume".to_string());
    }
    if let Some(ref p) = opts.prompt {
        let effective_prompt = build_review_prompt(p, opts.review);
        claude_args.push("-p".to_string());
        claude_args.push(effective_prompt);
        claude_args.push("--output-format".to_string());
        claude_args.push("stream-json".to_string());
        claude_args.push("--verbose".to_string());
    }

    // 7. Resolve env file if provided
    let mut resolved_env_vars = opts.env_vars.clone();
    if let Some(ref env_file_path) = opts.env_file {
        let content = std::fs::read_to_string(env_file_path)
            .with_context(|| format!("Failed to read env file: {}", env_file_path))?;

        let parsed: Vec<(String, String)> = content
            .lines()
            .filter(|line| {
                let t = line.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .filter_map(|line| {
                let t = line.trim();
                let (key, value) = t.split_once('=')?;
                let value = value.trim_matches('"').trim_matches('\'');
                Some((key.to_string(), value.to_string()))
            })
            .collect();

        let has_op_refs = parsed.iter().any(|(_, v)| v.starts_with("op://"));

        if has_op_refs {
            // Use `op run` to resolve op:// references
            let op_available = Command::new("op")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !op_available {
                bail!(
                    "env file contains op:// references but 1Password CLI (op) is not installed.\n  \
                     Install it: https://developer.1password.com/docs/cli/"
                );
            }

            println!("  ◇  Resolving op:// references via 1Password CLI...");

            let output = Command::new("op")
                .args([
                    "run",
                    &format!("--env-file={}", env_file_path),
                    "--no-masking",
                    "--",
                    "env",
                ])
                .output()
                .context("Failed to run `op run` to resolve secrets")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("1Password CLI failed to resolve secrets: {}", stderr);
            }

            // Parse resolved env output — only keep keys that were in our env file
            let env_keys: std::collections::HashSet<String> =
                parsed.iter().map(|(k, _)| k.clone()).collect();
            let resolved_output = String::from_utf8_lossy(&output.stdout);
            for line in resolved_output.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if env_keys.contains(key) {
                        resolved_env_vars.push(format!("{}={}", key, value));
                    }
                }
            }
        } else {
            // No op:// references, pass as-is
            for (key, value) in &parsed {
                resolved_env_vars.push(format!("{}={}", key, value));
            }
        }
    }

    // 8. Generate container name
    let short_hash: String = (0..6)
        .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
        .collect();
    let container_name = format!("vibepod-{}-{}", project_name, short_hash);

    // 9. Auth: load token
    let auth_manager = crate::auth::AuthManager::new(config_dir.clone());
    let home = crate::config::home_dir()?;
    let claude_json = home.join(".claude.json");

    let token_data = auth_manager
        .load_token()?
        .context("Not authenticated. Run `vibepod login` first.")?;

    if token_data.needs_renewal() {
        bail!("Token expires soon. Please run `vibepod login` to renew.");
    }

    // Add token as environment variable
    resolved_env_vars.push(format!("CLAUDE_CODE_OAUTH_TOKEN={}", token_data.token));

    // GitHub token: gh auth token でホスト側のトークンを自動取得
    if let Ok(output) = Command::new("gh").args(["auth", "token"]).output() {
        if output.status.success() {
            let gh_token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !gh_token.is_empty() {
                resolved_env_vars.push(format!("GH_TOKEN={}", gh_token));
            }
        }
    }

    // Copy .claude.json to temp file to protect host file from container writes
    let temp_claude_json = if claude_json.exists() {
        let temp_dir = std::env::temp_dir().join("vibepod-run");
        std::fs::create_dir_all(&temp_dir)?;
        let temp_path = temp_dir.join("claude.json");
        std::fs::copy(&claude_json, &temp_path)?;
        Some(temp_path)
    } else {
        None
    };

    Ok(Some(RunContext {
        runtime,
        container_name,
        effective_workspace,
        claude_args,
        resolved_env_vars,
        setup_cmd,
        temp_claude_json,
        global_config,
        home,
        worktree_branch_name,
        worktree_dir_name,
        lang_display,
        session_id,
        project_name,
    }))
}

async fn run_bridge(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
    let interactive = !opts.resume && opts.prompt.is_none();

    // bridge.env は常に config_dir/bridge.env から読む（--env-file はコンテナ用で独立）
    let config_dir = config::default_config_dir()?;
    let bridge_env_path = config_dir.join("bridge.env");

    if !bridge_env_path.exists() {
        bail!(
            "Bridge env file not found: {}\n  \
             Create it with SLACK_BOT_TOKEN, SLACK_APP_TOKEN, and SLACK_CHANNEL_ID.\n  \
             Default location: {}/bridge.env",
            bridge_env_path.display(),
            config_dir.display()
        );
    }

    let content = std::fs::read_to_string(&bridge_env_path).with_context(|| {
        format!(
            "Failed to read bridge env file: {}",
            bridge_env_path.display()
        )
    })?;

    let parsed: Vec<(String, String)> = content
        .lines()
        .filter(|line| {
            let t = line.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .filter_map(|line| {
            let t = line.trim();
            let (key, value) = t.split_once('=')?;
            let value = value.trim_matches('"').trim_matches('\'');
            Some((key.to_string(), value.to_string()))
        })
        .collect();

    let has_op_refs = parsed.iter().any(|(_, v)| v.starts_with("op://"));

    let resolved: std::collections::HashMap<String, String> = if has_op_refs {
        let op_available = Command::new("op")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !op_available {
            bail!(
                "bridge.env contains op:// references but 1Password CLI (op) is not installed.\n  \
                 Install it: https://developer.1password.com/docs/cli/"
            );
        }

        println!("  ◇  Resolving op:// references in bridge.env...");

        let output = Command::new("op")
            .args([
                "run",
                &format!("--env-file={}", bridge_env_path.display()),
                "--no-masking",
                "--",
                "env",
            ])
            .output()
            .context("Failed to run `op run` to resolve bridge secrets")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("1Password CLI failed to resolve bridge secrets: {}", stderr);
        }

        let env_keys: std::collections::HashSet<String> =
            parsed.iter().map(|(k, _)| k.clone()).collect();
        let resolved_output = String::from_utf8_lossy(&output.stdout);
        resolved_output
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once('=')?;
                if env_keys.contains(key) {
                    Some((key.to_string(), value.to_string()))
                } else {
                    None
                }
            })
            .collect()
    } else {
        parsed.into_iter().collect()
    };

    let slack_bot_token = resolved.get("SLACK_BOT_TOKEN").cloned().unwrap_or_default();
    let slack_app_token = resolved.get("SLACK_APP_TOKEN").cloned().unwrap_or_default();

    // SLACK_CHANNEL_ID: CLI option > env file
    let slack_channel_id = opts
        .slack_channel
        .clone()
        .or_else(|| resolved.get("SLACK_CHANNEL_ID").cloned())
        .unwrap_or_default();

    // Validate Slack channel ID format
    if !slack_channel_id.is_empty() && !validate_slack_channel_id(&slack_channel_id) {
        bail!(
            "Invalid Slack channel ID: '{}'. Channel IDs start with 'C' (e.g., C01ABC2DEF3).",
            slack_channel_id
        );
    }

    // LLM provider & API key
    let provider: crate::bridge::formatter::LlmProvider = opts.llm_provider.parse()?;
    let llm_api_key = provider
        .env_key_name()
        .and_then(|key| resolved.get(key).cloned())
        .unwrap_or_default();

    // Validation
    let mut missing = Vec::new();
    if slack_bot_token.is_empty() {
        missing.push("SLACK_BOT_TOKEN".to_string());
    }
    if slack_app_token.is_empty() {
        missing.push("SLACK_APP_TOKEN".to_string());
    }
    if slack_channel_id.is_empty() {
        missing.push("SLACK_CHANNEL_ID (set via --slack-channel or in bridge.env)".to_string());
    }
    if let Some(key_name) = provider.env_key_name() {
        if llm_api_key.is_empty() {
            missing.push(key_name.to_string());
        }
    }
    if !missing.is_empty() {
        bail!(
            "Bridge mode requires the following configuration:\n  - {}\n  \
             Set them in {} or use CLI options.",
            missing.join("\n  - "),
            bridge_env_path.display()
        );
    }

    println!(
        "  ◇  Bridge mode enabled (notify delay: {}s, llm: {:?})",
        opts.notify_delay, provider
    );
    if provider != crate::bridge::formatter::LlmProvider::None {
        println!(
            "  │  ⚠ Terminal output excerpts will be sent to {:?} API and Slack for formatting.",
            provider
        );
    } else {
        println!(
            "  │  Terminal output will be sent to Slack (local formatting, no LLM API calls)."
        );
    }

    let bridge_cfg = crate::bridge::BridgeConfig {
        slack_bot_token,
        slack_app_token,
        slack_channel_id,
        notify_delay_secs: opts.notify_delay,
        session_id: ctx.session_id.clone(),
        project_name: ctx.project_name.clone(),
        llm_provider: provider,
        llm_api_key,
    };

    let mode_label = if interactive {
        "bridge (interactive)"
    } else {
        "bridge (fire-and-forget)"
    };
    println!("  ◇  Starting container...");
    println!("  │  Agent: Claude Code");
    println!("  │  Mode: {}", mode_label);
    println!("  │  Mount: {} → /workspace", ctx.effective_workspace);
    if !ctx.lang_display.is_empty() {
        println!("  │  Language: {}", ctx.lang_display);
    }
    if opts.review {
        println!("  │  Review: enabled (GitHub Copilot)");
    }
    println!("  │");

    let container_config =
        build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);

    let container_id = ctx
        .runtime
        .create_and_start_container(&container_config)
        .await?;

    println!("  ◇  Container started: {}", ctx.container_name);
    println!("  │  Bridge mode active — Slack notifications enabled");
    println!("  └\n");

    let exit_code = crate::bridge::run(bridge_cfg, &ctx.runtime, &container_id).await?;

    // クリーンアップ
    ctx.runtime.stop_container(&container_id, 10).await.ok();
    ctx.runtime.remove_container(&container_id).await.ok();

    // temp .claude.json 削除
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    println!(
        "  Container stopped and removed. (exit code: {})",
        exit_code
    );
    Ok(())
}

async fn run_interactive(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
    println!("  ◇  Starting container...");
    println!("  │  Agent: Claude Code");
    println!("  │  Mode: interactive");
    println!("  │  Mount: {} → /workspace", ctx.effective_workspace);
    if !ctx.lang_display.is_empty() {
        println!("  │  Language: {}", ctx.lang_display);
    }
    println!("  │");

    let mut docker_args = vec![
        "run".to_string(),
        "-it".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        ctx.container_name.clone(),
        "-v".to_string(),
        format!("{}:/workspace", ctx.effective_workspace),
    ];
    // Mount host gitconfig for user identity
    let gitconfig = ctx.home.join(".gitconfig");
    if gitconfig.exists() {
        docker_args.push("-v".to_string());
        docker_args.push(format!(
            "{}:/home/vibepod/.gitconfig:ro",
            gitconfig.display()
        ));
    }
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        docker_args.push("-v".to_string());
        docker_args.push(format!("{}:/home/vibepod/.claude.json", temp_cj.display()));
    }
    if opts.no_network {
        docker_args.push("--network".to_string());
        docker_args.push("none".to_string());
    }
    for env_var in &ctx.resolved_env_vars {
        docker_args.push("-e".to_string());
        docker_args.push(env_var.clone());
    }
    docker_args.push("-e".to_string());
    docker_args.push("TERM=xterm-256color".to_string());
    docker_args.push(ctx.global_config.image.clone());
    if let Some(ref setup) = ctx.setup_cmd {
        docker_args.push("sh".to_string());
        docker_args.push("-c".to_string());
        docker_args.push(format!("{} && exec \"$@\"", setup));
        docker_args.push("sh".to_string());
    }
    docker_args.push("claude".to_string());
    docker_args.extend(ctx.claude_args.clone());

    println!("  ◇  Container: {}", ctx.container_name);
    println!("  └\n");

    let status = Command::new("docker")
        .args(&docker_args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to run container")?;

    if !status.success() {
        // Claude exited with non-zero, which is fine (e.g., user quit)
    }

    // Clean up temp claude.json
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    println!("  Container stopped and removed.");
    Ok(())
}

async fn run_fire_and_forget(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
    let mode_label = if opts.resume {
        "resume (--dangerously-skip-permissions)"
    } else {
        "fire-and-forget (--dangerously-skip-permissions)"
    };
    if opts.prompt.is_some() {
        println!("Starting container...");
        println!("Agent: Claude Code");
        println!("Mode: {}", mode_label);
        println!("Mount: {} → /workspace", ctx.effective_workspace);
        if !ctx.lang_display.is_empty() {
            println!("Language: {}", ctx.lang_display);
        }
        if opts.review {
            println!("Review: enabled (GitHub Copilot)");
        }
        println!();
    } else {
        println!("  ◇  Starting container...");
        println!("  │  Agent: Claude Code");
        println!("  │  Mode: {}", mode_label);
        println!("  │  Mount: {} → /workspace", ctx.effective_workspace);
        if !ctx.lang_display.is_empty() {
            println!("  │  Language: {}", ctx.lang_display);
        }
        println!("  │");
    }

    let container_config =
        build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);

    let container_id = ctx
        .runtime
        .create_and_start_container(&container_config)
        .await?;

    if opts.prompt.is_some() {
        println!("Container started: {}", ctx.container_name);
        println!("Press Ctrl+C to stop the container.");
        println!();
    } else {
        println!("  ◇  Container started: {}", ctx.container_name);
        println!("  │  Press Ctrl+C to stop the container.");
        println!("  └\n");
    }

    let result_text: Option<String> = tokio::select! {
        result = async {
            if opts.prompt.is_some() {
                ctx.runtime.stream_logs_formatted(&container_id).await
            } else {
                ctx.runtime.stream_logs(&container_id).await.map(|_| None)
            }
        } => {
            result.unwrap_or(None)
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping container...");
            None
        }
    };

    ctx.runtime.stop_container(&container_id, 10).await.ok();
    ctx.runtime.remove_container(&container_id).await.ok();

    // Clean up temp claude.json
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    if opts.prompt.is_some() {
        if let Some(ref text) = result_text {
            println!();
            println!("Result:");
            println!("{}", text);
        }

        // diff summary and worktree info
        let diff_dir = std::path::Path::new(&ctx.effective_workspace);
        let output = Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(diff_dir)
            .output()?;
        let stat = String::from_utf8_lossy(&output.stdout);
        if !stat.trim().is_empty() {
            println!();
            println!("Changes:");
            for line in stat.lines() {
                println!("{}", line);
            }
        }

        if let (Some(ref branch), Some(ref dir)) =
            (&ctx.worktree_branch_name, &ctx.worktree_dir_name)
        {
            println!();
            println!("Worktree: .worktrees/{}", dir);
            println!("Branch: {}", branch);
            println!("To review: cd .worktrees/{} && git diff main", dir);
            println!("To merge:  git merge {}", branch);
            println!("To remove: git worktree remove .worktrees/{}", dir);
        }

        println!();
        println!("Container stopped and removed.");
    } else {
        if let (Some(ref branch), Some(ref dir)) =
            (&ctx.worktree_branch_name, &ctx.worktree_dir_name)
        {
            println!("  ◇  Worktree: .worktrees/{}", dir);
            println!("  │  Branch: {}", branch);
            println!("  │  To review: cd .worktrees/{} && git diff main", dir);
            println!("  │  To merge:  git merge {}", branch);
            println!("  │  To remove: git worktree remove .worktrees/{}", dir);
        }

        println!("  Container stopped and removed.");
    }

    Ok(())
}

pub async fn execute(opts: RunOptions) -> Result<()> {
    let interactive = !opts.resume && opts.prompt.is_none();

    let Some(ctx) = prepare_context(&opts).await? else {
        return Ok(());
    };

    if opts.bridge {
        run_bridge(&opts, &ctx).await
    } else if interactive {
        run_interactive(&opts, &ctx).await
    } else {
        run_fire_and_forget(&opts, &ctx).await
    }
}
