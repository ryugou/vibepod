mod global;
mod projects;
mod vibepod_config;

pub use global::*;
pub use projects::*;
pub use vibepod_config::*;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Unified representation of ~/.config/vibepod/config.toml
#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct UnifiedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) global: Option<GlobalConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) projects: Vec<ProjectEntry>,
}

pub(crate) fn load_unified(config_dir: &Path) -> Result<UnifiedConfig> {
    let path = config_dir.join("config.toml");
    if !path.exists() {
        return Ok(UnifiedConfig::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let config: UnifiedConfig = toml::from_str(&content)?;
    Ok(config)
}

pub(crate) fn save_unified(config: &UnifiedConfig, config_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let path = config_dir.join("config.toml");

    // Load existing TOML as raw table to preserve unknown sections (e.g. [review], [run])
    let mut table: toml::value::Table = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        toml::from_str::<toml::value::Table>(&content).with_context(|| {
            format!(
                "Failed to parse {}: fix the syntax error first",
                path.display()
            )
        })?
    } else {
        toml::value::Table::new()
    };

    // Update [global] section only
    if let Some(ref global) = config.global {
        let global_value = toml::Value::try_from(global)?;
        table.insert("global".to_string(), global_value);
    } else {
        table.remove("global");
    }

    // Update [[projects]] section only
    if config.projects.is_empty() {
        table.remove("projects");
    } else {
        let projects_value = toml::Value::try_from(&config.projects)?;
        table.insert("projects".to_string(), projects_value);
    }

    let content = toml::to_string_pretty(&toml::Value::Table(table))?;
    std::fs::write(&path, content)?;
    Ok(())
}
