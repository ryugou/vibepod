use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// グローバル設定（default_agent, image）
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
    let mut unified = super::load_unified(config_dir)?;
    unified.global = Some(config.clone());
    super::save_unified(&unified, config_dir)
}

pub fn load_global_config(config_dir: &Path) -> Result<GlobalConfig> {
    // Try config.toml first
    let unified = super::load_unified(config_dir)?;
    if let Some(global) = unified.global {
        return Ok(global);
    }

    // Migration: if config.json exists, convert to config.toml
    let json_path = config_dir.join("config.json");
    if json_path.exists() {
        let json = fs::read_to_string(&json_path)
            .with_context(|| format!("Failed to read {}", json_path.display()))?;
        let config: GlobalConfig = serde_json::from_str(&json)?;
        save_global_config(&config, config_dir)?;
        fs::remove_file(&json_path).ok();
        return Ok(config);
    }

    anyhow::bail!(
        "Config not found: {}. Run `vibepod init` first.",
        config_dir.join("config.toml").display()
    )
}

pub fn home_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home))
}

pub fn default_config_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config").join("vibepod"))
}
