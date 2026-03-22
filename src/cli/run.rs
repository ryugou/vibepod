use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::config::{self, ProjectEntry};
use crate::runtime::{ContainerConfig, DockerRuntime};
use crate::ui::prompts;

pub async fn execute(
    resume: bool,
    prompt: Option<String>,
    no_network: bool,
    env_vars: Vec<String>,
) -> Result<()> {
    // Determine mode: interactive (default), prompt, or resume
    let interactive = !resume && prompt.is_none();

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

    // 6. Build claude args
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

    // 7. Generate container name
    let short_hash: String = (0..6)
        .map(|_| format!("{:x}", rand::random::<u8>() % 16))
        .collect();
    let container_name = format!("vibepod-{}-{}", project_name, short_hash);

    // Resolve paths
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let claude_dir = home.join(".claude");
    let claude_credentials = claude_dir.join(".credentials.json");
    let claude_json = home.join(".claude.json");

    if !claude_credentials.exists() {
        bail!("~/.claude/.credentials.json not found. Please run `claude` once to log in first.");
    }

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
            "-v".to_string(),
            format!(
                "{}:/home/node/.claude/.credentials.json:ro",
                claude_credentials.display()
            ),
        ];
        if claude_json.exists() {
            docker_args.push("-v".to_string());
            docker_args.push(format!("{}:/home/node/.claude.json", claude_json.display()));
        }
        if no_network {
            docker_args.push("--network".to_string());
            docker_args.push("none".to_string());
        }
        for env_var in &env_vars {
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

        println!("  Container stopped and removed.");
    } else {
        // Fire-and-forget mode: use bollard API
        let container_config = ContainerConfig {
            image: global_config.image,
            container_name: container_name.clone(),
            workspace_path: cwd_str,
            claude_credentials: claude_credentials.to_string_lossy().to_string(),
            claude_json: if claude_json.exists() {
                Some(claude_json.to_string_lossy().to_string())
            } else {
                None
            },
            args: {
                let mut full = vec!["claude".to_string()];
                full.extend(claude_args);
                full
            },
            env_vars,
            network_disabled: no_network,
        };

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
        println!("  Container stopped and removed.");
    }

    Ok(())
}
