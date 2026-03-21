use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub default_agent: String,
    pub image: String,
    pub claude_version: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            default_agent: "claude".to_string(),
            image: "vibepod-claude:latest".to_string(),
            claude_version: "latest".to_string(),
        }
    }
}

pub fn save_global_config(config: &GlobalConfig, config_dir: &Path) -> Result<()> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("Failed to create config dir: {}", config_dir.display()))?;
    let path = config_dir.join("config.json");
    let json = serde_json::to_string_pretty(config)?;
    fs::write(&path, json)
        .with_context(|| format!("Failed to write config: {}", path.display()))?;
    Ok(())
}

pub fn load_global_config(config_dir: &Path) -> Result<GlobalConfig> {
    let path = config_dir.join("config.json");
    let json = fs::read_to_string(&path).with_context(|| {
        format!(
            "Config not found: {}. Run `vibepod init` first.",
            path.display()
        )
    })?;
    let config: GlobalConfig = serde_json::from_str(&json)?;
    Ok(config)
}

pub fn default_config_dir() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".vibepod"))
}
