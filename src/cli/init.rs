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
