use anyhow::{Context, Result};
use std::process::Command;

use super::{build_container_config, RunContext, RunOptions};

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

    let container_config =
        build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
    let docker_args = container_config.to_docker_args(true);

    // Record session now that container is about to start
    ctx.store.add(ctx.deferred_session.clone())?;

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
