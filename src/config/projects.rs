use anyhow::Result;
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
    // Once config.toml exists it is authoritative; its projects list wins even when empty
    let toml_path = config_dir.join("config.toml");
    if toml_path.exists() {
        let unified = super::load_unified(config_dir)?;
        return Ok(ProjectsConfig {
            projects: unified.projects,
        });
    }

    // Migration: if projects.json exists and config.toml does not, convert to config.toml
    let json_path = config_dir.join("projects.json");
    if json_path.exists() {
        let json = fs::read_to_string(&json_path)?;
        let config: ProjectsConfig = serde_json::from_str(&json)?;
        if !config.projects.is_empty() {
            save_projects(&config, config_dir)?;
            fs::remove_file(&json_path).ok();
        }
        return Ok(config);
    }

    Ok(ProjectsConfig::default())
}

pub fn save_projects(config: &ProjectsConfig, config_dir: &Path) -> Result<()> {
    let mut unified = super::load_unified(config_dir)?;
    unified.projects = config.projects.clone();
    super::save_unified(&unified, config_dir)
}

pub fn is_project_registered(config: &ProjectsConfig, project_path: &str) -> bool {
    config.projects.iter().any(|p| p.path == project_path)
}

pub fn register_project(config: &mut ProjectsConfig, entry: ProjectEntry) {
    if !is_project_registered(config, &entry.path) {
        config.projects.push(entry);
    }
}
