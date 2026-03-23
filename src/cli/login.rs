use anyhow::{bail, Context, Result};

use crate::auth::{self, AuthManager, TokenData};
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

    let auth_manager = AuthManager::new(config_dir.clone());

    if let Some(existing) = auth_manager.load_token()? {
        if !existing.is_expired() {
            println!(
                "  ⚠  Existing token found (valid until {}).",
                chrono::DateTime::parse_from_rfc3339(&existing.expires_at)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|_| existing.expires_at.clone())
            );
            if !dialoguer::Confirm::new()
                .with_prompt("  Overwrite?")
                .default(false)
                .interact()?
            {
                println!("  └\n");
                return Ok(());
            }
        }
    }

    println!("  ◇  Creating long-lived token for container use...");
    println!("  │");

    let token = auth::run_setup_token(&global_config.image)?;

    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::days(365);
    let token_data = TokenData {
        token,
        created_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    auth_manager.save_token(&token_data)?;

    println!("  │");
    println!("  ◇  Login successful! Token saved.");
    println!("  │");
    println!("  │  Run `vibepod run` in any git repo to start.");
    println!("  └\n");

    Ok(())
}
