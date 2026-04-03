use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::runtime::{format_stream_event, StreamEvent};

use super::{build_container_config, RunContext, RunOptions};

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

    let result_text: Option<String> = tokio::select! {
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
            rt
        } => r,
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping container...");
            None
        }
    };

    let _ = log_child.kill().await;

    // docker stop + docker rm -f
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
