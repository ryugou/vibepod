use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::runtime::DockerRuntime;

use super::{build_container_config, RunContext, RunOptions};

/// For a new reuse container that has a `setup_cmd`, follow docker logs until
/// `VIBEPOD_SETUP_DONE` appears, printing each line so the user can see progress.
async fn wait_for_reuse_setup(container_name: &str) -> Result<()> {
    use tokio::io::AsyncBufReadExt;

    let mut child = tokio::process::Command::new("docker")
        .args(["logs", "--follow", container_name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .context("Failed to follow setup logs")?;

    let stdout = child
        .stdout
        .take()
        .context("Failed to capture setup logs")?;
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        println!("{}", line);
        if line.contains("VIBEPOD_SETUP_DONE") {
            break;
        }
    }

    let _ = child.kill().await;
    Ok(())
}

pub(super) async fn run_interactive(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
    println!("  ◇  Starting container...");
    println!("  │  Agent: Claude Code");
    println!("  │  Mode: interactive");
    println!("  │  Mount: {} → /workspace", ctx.effective_workspace);
    for (host, container) in &ctx.extra_mounts {
        println!("  │  Mount (ro): {} → {}", host, container);
    }
    if !ctx.lang_display.is_empty() {
        println!("  │  Language: {}", ctx.lang_display);
    }
    println!("  │");

    // Record session now that container is about to start
    ctx.store.add(ctx.deferred_session.clone())?;

    println!("  ◇  Container: {}", ctx.container_name);
    println!("  └\n");

    if ctx.reuse {
        if ctx.reuse_existing {
            // Container was stopped; restart its idle entrypoint (tail -f /dev/null)
            let runtime = DockerRuntime::new().await?;
            runtime.start_container(&ctx.container_name).await?;
        } else {
            // First run: create the container with an idle entrypoint so subsequent
            // runs can attach without re-running setup.
            let container_config =
                build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
            let docker_run_args = container_config.to_docker_args(false);

            let output = Command::new("docker")
                .args(&docker_run_args)
                .output()
                .context("Failed to create reuse container")?;

            if !output.status.success() {
                bail!(
                    "Failed to create reuse container: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }

            // If setup_cmd was included, wait until it finishes before running claude
            if ctx.setup_cmd.is_some() {
                wait_for_reuse_setup(&ctx.container_name).await?;
            }
        }

        // Run claude via docker exec -it (container is now running the idle entrypoint)
        let mut exec_args = vec![
            "exec".to_string(),
            "-it".to_string(),
            ctx.container_name.clone(),
            "claude".to_string(),
        ];
        exec_args.extend(ctx.claude_args.clone());

        let status = Command::new("docker")
            .args(&exec_args)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to exec into container")?;

        if !status.success() {
            // Claude exited with non-zero, which is fine (e.g., user quit)
        }
    } else {
        let container_config =
            build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
        let docker_args = container_config.to_docker_args(true);

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
    }

    // Clean up temp claude.json
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    if ctx.reuse {
        println!("  Container stopped (reuse mode: container preserved).");
    } else {
        println!("  Container stopped and removed.");
    }
    Ok(())
}
