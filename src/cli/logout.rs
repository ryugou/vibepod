use anyhow::Result;

use crate::auth::AuthManager;
use crate::config;

pub fn execute(all: bool) -> Result<()> {
    println!("\n  ┌  VibePod Logout");
    println!("  │");

    let config_dir = config::default_config_dir()?;
    let cwd = std::env::current_dir()?;
    let auth_manager = AuthManager::new(config_dir, cwd);

    auth_manager.delete_shared()?;
    println!("  ◇  共有セッションを削除しました");

    if all {
        auth_manager.delete_all_isolated()?;
        println!("  ◇  全てのコンテナ専用セッションを削除しました");
    }

    println!("  └\n");
    Ok(())
}
