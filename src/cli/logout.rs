use anyhow::Result;

use crate::auth::AuthManager;
use crate::config;

pub fn execute() -> Result<()> {
    println!("\n  ┌  VibePod Logout");
    println!("  │");

    let config_dir = config::default_config_dir()?;
    let auth_manager = AuthManager::new(config_dir);

    auth_manager.delete_token()?;
    println!("  ◇  トークンを削除しました");

    println!("  └\n");
    Ok(())
}
