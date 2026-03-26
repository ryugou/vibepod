use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub default_agent: String,
    pub image: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            default_agent: "claude".to_string(),
            image: "vibepod-claude:latest".to_string(),
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

pub fn home_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home))
}

pub fn default_config_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config").join("vibepod"))
}
