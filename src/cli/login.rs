use anyhow::{bail, Context, Result};

use crate::auth::{self, AuthManager};
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

    let cwd = std::env::current_dir()?;
    let auth_manager = AuthManager::new(config_dir.clone(), cwd);

    if let Some(existing) = auth_manager.load_shared()? {
        if !existing.is_expired() {
            println!("  ⚠  既存のセッションがあります。");
            if !dialoguer::Confirm::new()
                .with_prompt("  上書きしますか？")
                .default(false)
                .interact()?
            {
                println!("  └\n");
                return Ok(());
            }
        }
    }

    println!("  ◇  コンテナ用の認証セッションを作成します");
    println!("  │");

    let creds = auth::run_login_flow(&global_config.image)?;
    auth_manager.save_shared(&creds)?;

    println!("  │");
    println!("  ◇  認証完了！");
    println!("  │  セッションを保存しました: ~/.config/vibepod/auth/credentials.json");
    println!("  └\n");

    Ok(())
}
