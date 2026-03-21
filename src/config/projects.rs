use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub name: String,
    pub path: String,
    pub remote: Option<String>,
    pub registered_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectsConfig {
    pub projects: Vec<ProjectEntry>,
}

pub fn load_projects(config_dir: &Path) -> Result<ProjectsConfig> {
    let path = config_dir.join("projects.json");
    if !path.exists() {
        return Ok(ProjectsConfig::default());
    }
    let json = fs::read_to_string(&path)?;
    let config: ProjectsConfig = serde_json::from_str(&json)?;
    Ok(config)
}

pub fn save_projects(config: &ProjectsConfig, config_dir: &Path) -> Result<()> {
    fs::create_dir_all(config_dir)?;
    let path = config_dir.join("projects.json");
    let json = serde_json::to_string_pretty(config)?;
    fs::write(&path, json)?;
    Ok(())
}

pub fn is_project_registered(config: &ProjectsConfig, project_path: &str) -> bool {
    config.projects.iter().any(|p| p.path == project_path)
}

pub fn register_project(config: &mut ProjectsConfig, entry: ProjectEntry) {
    if !is_project_registered(config, &entry.path) {
        config.projects.push(entry);
    }
}
