use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::config::{self, ProjectEntry};
use crate::git;
use crate::runtime::{ContainerConfig, DockerRuntime};
use crate::session::{self, SessionStore};
use crate::ui::prompts;

/// ContainerConfig を構築するヘルパー関数（bridge / fire-and-forget で共有）
fn build_container_config(
    image: String,
    container_name: String,
    workspace_path: String,
    claude_json: Option<String>,
    claude_args: Vec<String>,
    home: &std::path::Path,
    env_vars: Vec<String>,
    network_disabled: bool,
) -> ContainerConfig {
    let gitconfig = home.join(".gitconfig");
    ContainerConfig {
        image,
        container_name,
        workspace_path,
        claude_json,
        args: {
            let mut full = vec!["claude".to_string()];
            full.extend(claude_args);
            full
        },
        gitconfig: if gitconfig.exists() {
            Some(gitconfig.to_string_lossy().to_string())
        } else {
            None
        },
        env_vars,
        network_disabled,
    }
}

pub async fn execute(
    resume: bool,
    prompt: Option<String>,
    no_network: bool,
    env_vars: Vec<String>,
    env_file: Option<String>,
    bridge: bool,
    notify_delay: u64,
    slack_channel: Option<String>,
) -> Result<()> {
    // Determine mode: interactive (default), prompt, or resume
    let interactive = !resume && prompt.is_none();

    // 1. Check git repo
    let cwd = std::env::current_dir()?;
    if !git::is_git_repo(&cwd) {
        bail!("Not a git repository. Run this command inside a git-initialized directory.");
    }

    // Record session for restore
    let head_before = git::get_head_hash(&cwd)?;
    let current_branch = git::get_current_branch(&cwd).unwrap_or_else(|_| "unknown".to_string());

    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir.clone());

    // Ensure .vibepod/ is in .gitignore
    let gitignore_path = cwd.join(".gitignore");
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if !content
            .lines()
            .any(|l| l.trim() == ".vibepod/" || l.trim() == ".vibepod")
        {
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

    // 6. Bridge env validation (before Docker startup)
    let bridge_config = if bridge {
        let config_dir_path = config::default_config_dir()?;
        let default_bridge_env = config_dir_path.join("bridge.env");
        let bridge_env_path = env_file
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or(default_bridge_env);

        if !bridge_env_path.exists() {
            bail!(
                "Bridge env file not found: {}\n  \
                 Create it with SLACK_BOT_TOKEN, SLACK_APP_TOKEN, and SLACK_CHANNEL_ID.\n  \
                 Default location: {}/bridge.env",
                bridge_env_path.display(),
                config_dir_path.display()
            );
        }

        let content = std::fs::read_to_string(&bridge_env_path)
            .with_context(|| format!("Failed to read bridge env file: {}", bridge_env_path.display()))?;

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
        let slack_channel_id = slack_channel
            .clone()
            .or_else(|| resolved.get("SLACK_CHANNEL_ID").cloned())
            .unwrap_or_default();

        // Validation
        let mut missing = Vec::new();
        if slack_bot_token.is_empty() {
            missing.push("SLACK_BOT_TOKEN");
        }
        if slack_app_token.is_empty() {
            missing.push("SLACK_APP_TOKEN");
        }
        if slack_channel_id.is_empty() {
            missing.push("SLACK_CHANNEL_ID (set via --slack-channel or in bridge.env)");
        }
        if !missing.is_empty() {
            bail!(
                "Bridge mode requires the following configuration:\n  - {}\n  \
                 Set them in {} or use CLI options.",
                missing.join("\n  - "),
                bridge_env_path.display()
            );
        }

        println!("  ◇  Bridge mode enabled (notify delay: {}s)", notify_delay);

        Some(crate::bridge::BridgeConfig {
            slack_bot_token,
            slack_app_token,
            slack_channel_id,
            notify_delay_secs: notify_delay,
            session_id: session_id.clone(),
            project_name: project_name.clone(),
        })
    } else {
        None
    };

    // 7. Build claude args
    let mut claude_args: Vec<String> = Vec::new();
    if !interactive {
        claude_args.push("--dangerously-skip-permissions".to_string());
    }
    if resume {
        claude_args.push("--resume".to_string());
    }
    if let Some(ref p) = prompt {
        claude_args.push("-p".to_string());
        claude_args.push(p.clone());
    }

    // 8. Resolve env file if provided
    let mut resolved_env_vars = env_vars.clone();
    if let Some(ref env_file_path) = env_file {
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

    // 9. Generate container name
    let short_hash: String = (0..6)
        .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
        .collect();
    let container_name = format!("vibepod-{}-{}", project_name, short_hash);

    // 10. Auth: load token
    let auth_manager = crate::auth::AuthManager::new(config_dir.clone());
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let claude_json = home.join(".claude.json");

    let token_data = auth_manager
        .load_token()?
        .context("Not authenticated. Run `vibepod login` first.")?;

    if token_data.needs_renewal() {
        bail!("Token expires soon. Please run `vibepod login` to renew.");
    }

    // Add token as environment variable
    resolved_env_vars.push(format!("CLAUDE_CODE_OAUTH_TOKEN={}", token_data.token));

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

    // === Bridge mode: early return ===
    if let Some(bridge_cfg) = bridge_config {
        let mode_label = if interactive {
            "bridge (interactive)"
        } else {
            "bridge (fire-and-forget)"
        };
        println!("  ◇  Starting container...");
        println!("  │  Agent: Claude Code");
        println!("  │  Mode: {}", mode_label);
        println!("  │  Mount: {} → /workspace", cwd.display());
        println!("  │");

        let container_config = build_container_config(
            global_config.image,
            container_name.clone(),
            cwd_str,
            temp_claude_json
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            claude_args,
            &home,
            resolved_env_vars,
            no_network,
        );

        let container_id = runtime
            .create_and_start_container(&container_config)
            .await?;

        println!("  ◇  Container started: {}", container_name);
        println!("  │  Bridge mode active — Slack notifications enabled");
        println!("  └\n");

        let exit_code = crate::bridge::run(bridge_cfg, &runtime, &container_id).await?;

        // クリーンアップ
        runtime.stop_container(&container_id, 10).await.ok();
        runtime.remove_container(&container_id).await.ok();

        // temp .claude.json 削除
        if let Some(ref temp_cj) = temp_claude_json {
            std::fs::remove_file(temp_cj).ok();
        }

        println!("  Container stopped and removed. (exit code: {})", exit_code);
        return Ok(());
    }

    // === Existing interactive / fire-and-forget paths ===
    let mode_label = if interactive {
        "interactive"
    } else if resume {
        "resume (--dangerously-skip-permissions)"
    } else {
        "fire-and-forget (--dangerously-skip-permissions)"
    };
    println!("  ◇  Starting container...");
    println!("  │  Agent: Claude Code");
    println!("  │  Mode: {}", mode_label);
    println!("  │  Mount: {} → /workspace", cwd.display());
    println!("  │");

    if interactive {
        // Interactive mode: docker run -it with inherited stdio
        let mut docker_args = vec![
            "run".to_string(),
            "-it".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "-v".to_string(),
            format!("{}:/workspace", cwd_str),
        ];
        // Mount host gitconfig for user identity
        let gitconfig = home.join(".gitconfig");
        if gitconfig.exists() {
            docker_args.push("-v".to_string());
            docker_args.push(format!(
                "{}:/home/vibepod/.gitconfig:ro",
                gitconfig.display()
            ));
        }
        if let Some(ref temp_cj) = temp_claude_json {
            docker_args.push("-v".to_string());
            docker_args.push(format!("{}:/home/vibepod/.claude.json", temp_cj.display()));
        }
        if no_network {
            docker_args.push("--network".to_string());
            docker_args.push("none".to_string());
        }
        for env_var in &resolved_env_vars {
            docker_args.push("-e".to_string());
            docker_args.push(env_var.clone());
        }
        docker_args.push("-e".to_string());
        docker_args.push("TERM=xterm-256color".to_string());
        docker_args.push(global_config.image.clone());
        docker_args.push("claude".to_string());
        docker_args.extend(claude_args);

        println!("  ◇  Container: {}", container_name);
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
        if let Some(ref temp_cj) = temp_claude_json {
            std::fs::remove_file(temp_cj).ok();
        }

        println!("  Container stopped and removed.");
    } else {
        // Fire-and-forget mode: use bollard API
        let container_config = build_container_config(
            global_config.image,
            container_name.clone(),
            cwd_str,
            temp_claude_json
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            claude_args,
            &home,
            resolved_env_vars,
            no_network,
        );

        let container_id = runtime
            .create_and_start_container(&container_config)
            .await?;

        println!("  ◇  Container started: {}", container_name);
        println!("  │  Press Ctrl+C to stop the container.");
        println!("  └\n");

        tokio::select! {
            _ = runtime.stream_logs(&container_id) => {
                // Agent finished naturally
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n  Stopping container...");
            }
        }

        runtime.stop_container(&container_id, 10).await.ok();
        runtime.remove_container(&container_id).await.ok();

        // Clean up temp claude.json
        if let Some(ref temp_cj) = temp_claude_json {
            std::fs::remove_file(temp_cj).ok();
        }

        println!("  Container stopped and removed.");
    }

    Ok(())
}
