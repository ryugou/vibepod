use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::runtime::{format_stream_event, StreamEvent};

use super::{build_container_config, RunContext, RunOptions};

/// For a new reuse container that has a `setup_cmd`, follow docker logs until
/// `VIBEPOD_SETUP_DONE` appears.
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

    let mut found_marker = false;
    while let Ok(Some(line)) = lines.next_line().await {
        println!("{}", line);
        if line.contains("VIBEPOD_SETUP_DONE") {
            found_marker = true;
            break;
        }
    }

    let _ = child.kill().await;
    if !found_marker {
        bail!("Container setup failed: VIBEPOD_SETUP_DONE marker was not found. Check the setup output above for errors.");
    }
    Ok(())
}

pub(super) async fn run_fire_and_forget(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
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
        for (host, container) in &ctx.extra_mounts {
            println!("Mount (ro): {} → {}", host, container);
        }
        if !ctx.lang_display.is_empty() {
            println!("Language: {}", ctx.lang_display);
        }
        if !ctx.reviewers.is_empty() {
            println!("Review: enabled ({})", ctx.reviewers.join(", "));
        }
        println!();
    } else {
        println!("  ◇  Starting container...");
        println!("  │  Agent: Claude Code");
        println!("  │  Mode: {}", mode_label);
        println!("  │  Mount: {} → /workspace", ctx.effective_workspace);
        for (host, container) in &ctx.extra_mounts {
            println!("  │  Mount (ro): {} → {}", host, container);
        }
        if !ctx.lang_display.is_empty() {
            println!("  │  Language: {}", ctx.lang_display);
        }
        println!("  │");
    }

    // For reuse mode: ensure the container is running its idle entrypoint,
    // then run claude via docker exec (output captured directly from exec stdout).
    if ctx.reuse {
        return run_reuse_prompt(opts, ctx, mode_label).await;
    }

    let container_config =
        build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
    let docker_run_args = container_config.to_docker_args(false);

    // Start container with docker run -d
    let start_output = Command::new("docker")
        .args(&docker_run_args)
        .output()
        .context("Failed to start container")?;

    if !start_output.status.success() {
        bail!(
            "Failed to start container: {}",
            String::from_utf8_lossy(&start_output.stderr).trim()
        );
    }

    // Record session now that container has actually started
    ctx.store.add(ctx.deferred_session.clone())?;

    if opts.prompt.is_some() {
        println!("Container started: {}", ctx.container_name);
        println!("Press Ctrl+C to stop the container.");
        println!();
    } else {
        println!("  ◇  Container started: {}", ctx.container_name);
        println!("  │  Press Ctrl+C to stop the container.");
        println!("  └\n");
    }

    let separator = "────────────────────────────────────────────────────────";
    if opts.prompt.is_some() {
        println!("{}", separator);
    }

    // Stream logs with docker logs --follow
    let mut log_child = tokio::process::Command::new("docker")
        .args(["logs", "--follow", &ctx.container_name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .context("Failed to run docker logs")?;

    let stdout = log_child
        .stdout
        .take()
        .context("Failed to capture docker logs stdout")?;

    let is_prompt = opts.prompt.is_some();
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = tokio::io::AsyncBufReadExt::lines(reader);

    let (result_text, ctrl_c_pressed): (Option<String>, bool) = tokio::select! {
        r = async {
            let mut rt: Option<String> = None;
            while let Ok(Some(line)) = lines.next_line().await {
                if is_prompt {
                    match format_stream_event(&line) {
                        StreamEvent::Display(s) => println!("{}", s),
                        StreamEvent::Result(s) => rt = Some(s),
                        StreamEvent::Skip => {}
                        StreamEvent::PassThrough(s) => println!("{}", s),
                    }
                } else {
                    println!("{}", line);
                }
            }
            (rt, false)
        } => r,
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping container...");
            (None, true)
        }
    };

    // Kill the log child first so wait() doesn't block
    let _ = log_child.kill().await;
    let exit_status = log_child.wait().await;

    // Check docker logs exit status — a non-zero exit that isn't from Ctrl+C is an error
    if !ctrl_c_pressed {
        if let Ok(status) = exit_status {
            // Signal-killed processes (e.g., after our kill()) have no exit code on Unix
            if let Some(code) = status.code() {
                if code != 0 {
                    bail!(
                        "docker logs exited with code {} for container {}",
                        code,
                        ctx.container_name
                    );
                }
            }
            // No exit code (killed by signal) is expected after kill()
        }
    }

    Command::new("docker")
        .args(["stop", "-t", "10", &ctx.container_name])
        .output()
        .ok();
    Command::new("docker")
        .args(["rm", "-f", &ctx.container_name])
        .output()
        .ok();

    if opts.prompt.is_some() {
        println!("{}", separator);
    }

    // Clean up temp claude.json
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    print_post_run_summary(opts, ctx, result_text.as_deref(), false);
    Ok(())
}

/// Run fire-and-forget mode for a `--reuse` container.
/// The container uses an idle entrypoint (`tail -f /dev/null`); claude is run
/// via `docker exec` so that subsequent runs can reuse the same container without
/// re-executing setup.
async fn run_reuse_prompt(opts: &RunOptions, ctx: &RunContext, _mode_label: &str) -> Result<()> {
    if ctx.reuse_existing {
        // Container was stopped; restart its idle entrypoint
        let start = Command::new("docker")
            .args(["start", &ctx.container_name])
            .output()
            .context("Failed to start reuse container")?;
        if !start.status.success() {
            bail!(
                "Failed to start reuse container: {}",
                String::from_utf8_lossy(&start.stderr).trim()
            );
        }
    } else {
        // First run: create the container with an idle entrypoint
        let container_config =
            build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
        let docker_run_args = container_config.to_docker_args(false);

        let start_output = Command::new("docker")
            .args(&docker_run_args)
            .output()
            .context("Failed to start container")?;

        if !start_output.status.success() {
            bail!(
                "Failed to start container: {}",
                String::from_utf8_lossy(&start_output.stderr).trim()
            );
        }

        // Wait for setup to finish before running claude
        if ctx.setup_cmd.is_some() {
            wait_for_reuse_setup(&ctx.container_name).await?;
        }
    }

    ctx.store.add(ctx.deferred_session.clone())?;

    if opts.prompt.is_some() {
        println!("Container started: {}", ctx.container_name);
        println!("Press Ctrl+C to stop the container.");
        println!();
    } else {
        println!("  ◇  Container started: {}", ctx.container_name);
        println!("  │  Press Ctrl+C to stop the container.");
        println!("  └\n");
    }

    let separator = "────────────────────────────────────────────────────────";
    if opts.prompt.is_some() {
        println!("{}", separator);
    }

    // Run claude via docker exec and capture output directly
    let mut exec_child = tokio::process::Command::new("docker")
        .arg("exec")
        .arg(&ctx.container_name)
        .arg("claude")
        .args(&ctx.claude_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .context("Failed to exec claude in container")?;

    let stdout = exec_child
        .stdout
        .take()
        .context("Failed to capture exec stdout")?;

    let is_prompt = opts.prompt.is_some();
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = tokio::io::AsyncBufReadExt::lines(reader);

    let (result_text, _ctrl_c_pressed): (Option<String>, bool) = tokio::select! {
        r = async {
            let mut rt: Option<String> = None;
            while let Ok(Some(line)) = lines.next_line().await {
                if is_prompt {
                    match format_stream_event(&line) {
                        StreamEvent::Display(s) => println!("{}", s),
                        StreamEvent::Result(s) => rt = Some(s),
                        StreamEvent::Skip => {}
                        StreamEvent::PassThrough(s) => println!("{}", s),
                    }
                } else {
                    println!("{}", line);
                }
            }
            (rt, false)
        } => r,
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping container...");
            (None, true)
        }
    };

    let _ = exec_child.kill().await;
    let _ = exec_child.wait().await;

    // Stop the container (but preserve it) so the next run --reuse
    // finds it as a stopped container and can quickly restart it.
    Command::new("docker")
        .args(["stop", "-t", "10", &ctx.container_name])
        .output()
        .ok();
    // Container is preserved (not removed) for the next vibepod run --reuse

    if opts.prompt.is_some() {
        println!("{}", separator);
    }

    // Clean up temp claude.json
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    print_post_run_summary(opts, ctx, result_text.as_deref(), true);
    Ok(())
}

fn print_post_run_summary(
    opts: &RunOptions,
    ctx: &RunContext,
    result_text: Option<&str>,
    reuse: bool,
) {
    let stopped_msg = if reuse {
        "Container stopped (reuse mode: container preserved)."
    } else {
        "Container stopped and removed."
    };

    if opts.prompt.is_some() {
        if let Some(text) = result_text {
            println!();
            println!("Result:");
            println!("{}", text);
        }

        // diff summary and worktree info
        let diff_dir = std::path::Path::new(&ctx.effective_workspace);
        if let Ok(output) = Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(diff_dir)
            .output()
        {
            let stat = String::from_utf8_lossy(&output.stdout);
            if !stat.trim().is_empty() {
                println!();
                println!("Changes:");
                for line in stat.lines() {
                    println!("{}", line);
                }
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
        println!("{}", stopped_msg);
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

        println!("  {}", stopped_msg);
    }
}
