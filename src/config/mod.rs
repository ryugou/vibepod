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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ecc: Option<EccConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EccConfig {
    #[serde(default = "default_ecc_repo")]
    pub repo: String,
    #[serde(default = "default_ecc_ref", rename = "ref")]
    pub r#ref: String,
    #[serde(default = "default_refresh_ttl")]
    pub refresh_ttl: String,
    #[serde(default = "default_auto_refresh")]
    pub auto_refresh: bool,
}

fn default_ecc_repo() -> String {
    "https://github.com/affaan-m/everything-claude-code".to_string()
}
fn default_ecc_ref() -> String {
    "main".to_string()
}
fn default_refresh_ttl() -> String {
    "24h".to_string()
}
fn default_auto_refresh() -> bool {
    true
}

impl Default for EccConfig {
    fn default() -> Self {
        Self {
            repo: default_ecc_repo(),
            r#ref: default_ecc_ref(),
            refresh_ttl: default_refresh_ttl(),
            auto_refresh: default_auto_refresh(),
        }
    }
}

impl EccConfig {
    /// Parse the configured TTL string into seconds.
    ///
    /// In practice this never falls back: `load_unified` calls
    /// `validate()` which fails fast on unparseable TTLs. The
    /// `unwrap_or` is defensive for hypothetical programmatic
    /// constructions of `EccConfig` that bypass validation.
    pub fn refresh_ttl_seconds(&self) -> u64 {
        parse_duration_to_seconds(&self.refresh_ttl).unwrap_or(24 * 60 * 60)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let ttl = parse_duration_to_seconds(&self.refresh_ttl).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid [ecc] refresh_ttl '{}': expected duration like '24h', '30m', '1d', or '2h30m'; mixed units must fully specify each unit (e.g. '2h30' is invalid — use '2h30m')",
                self.refresh_ttl
            )
        })?;
        if ttl == 0 {
            anyhow::bail!("invalid [ecc] refresh_ttl: must be > 0");
        }
        if self.repo.trim().is_empty() {
            anyhow::bail!("invalid [ecc] repo: must not be empty");
        }
        if self.r#ref.trim().is_empty() {
            anyhow::bail!("invalid [ecc] ref: must not be empty");
        }
        Ok(())
    }
}

/// Parse "24h" / "30m" / "90s" / "2h30m" -> total seconds.
pub(crate) fn parse_duration_to_seconds(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    let mut num: u64 = 0;
    let mut seen_digit = false;
    for c in s.chars() {
        if let Some(d) = c.to_digit(10) {
            num = num.checked_mul(10)?.checked_add(d as u64)?;
            seen_digit = true;
        } else {
            if !seen_digit {
                return None;
            }
            let multiplier = match c {
                's' => 1,
                'm' => 60,
                'h' => 60 * 60,
                'd' => 24 * 60 * 60,
                _ => return None,
            };
            total = total.checked_add(num.checked_mul(multiplier)?)?;
            num = 0;
            seen_digit = false;
        }
    }
    if seen_digit {
        return None;
    }
    Some(total)
}

/// Load the `[ecc]` section from `<config_dir>/config.toml`, falling back
/// to `EccConfig::default()` when the section is absent or the file
/// does not exist.
pub fn load_ecc_config(config_dir: &Path) -> Result<EccConfig> {
    let unified = load_unified(config_dir)?;
    Ok(unified.ecc.unwrap_or_default())
}

pub(crate) fn load_unified(config_dir: &Path) -> Result<UnifiedConfig> {
    let path = config_dir.join("config.toml");
    if !path.exists() {
        return Ok(UnifiedConfig::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let config: UnifiedConfig = toml::from_str(&content)?;
    if let Some(ref ecc) = config.ecc {
        ecc.validate()?;
    }
    Ok(config)
}

pub(crate) fn save_unified(config: &UnifiedConfig, config_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let path = config_dir.join("config.toml");

    // Load existing TOML as raw table to preserve unknown sections (e.g. [run])
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

    // Update [ecc] section only
    if let Some(ref ecc) = config.ecc {
        table.insert("ecc".to_string(), toml::Value::try_from(ecc)?);
    } else {
        table.remove("ecc");
    }

    let content = toml::to_string_pretty(&toml::Value::Table(table))?;
    std::fs::write(&path, content)?;
    Ok(())
}

#[cfg(test)]
mod ecc_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ecc_config_defaults_when_section_missing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "").unwrap();
        let unified = load_unified(dir.path()).unwrap();
        let ecc = unified.ecc.unwrap_or_default();
        assert_eq!(
            ecc.repo,
            "https://github.com/affaan-m/everything-claude-code"
        );
        assert_eq!(ecc.r#ref, "main");
        assert_eq!(ecc.refresh_ttl_seconds(), 24 * 60 * 60);
        assert!(ecc.auto_refresh);
    }

    #[test]
    fn ecc_config_parses_all_fields() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[ecc]
repo = "https://example.com/fork.git"
ref = "v1.10.0"
refresh_ttl = "2h"
auto_refresh = false
"#,
        )
        .unwrap();
        let unified = load_unified(dir.path()).unwrap();
        let ecc = unified.ecc.unwrap();
        assert_eq!(ecc.repo, "https://example.com/fork.git");
        assert_eq!(ecc.r#ref, "v1.10.0");
        assert_eq!(ecc.refresh_ttl_seconds(), 2 * 60 * 60);
        assert!(!ecc.auto_refresh);
    }

    #[test]
    fn ecc_config_rejects_invalid_ttl() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[ecc]
refresh_ttl = "garbage"
"#,
        )
        .unwrap();
        let err = load_unified(dir.path()).unwrap_err();
        assert!(
            format!("{err}").contains("refresh_ttl"),
            "expected ttl error, got: {err}"
        );
    }

    #[test]
    fn ecc_config_rejects_empty_repo() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[ecc]
repo = ""
"#,
        )
        .unwrap();
        let err = load_unified(dir.path()).unwrap_err();
        assert!(format!("{err}").contains("repo"), "got: {err}");
    }

    #[test]
    fn ecc_config_rejects_empty_ref() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[ecc]
ref = ""
"#,
        )
        .unwrap();
        let err = load_unified(dir.path()).unwrap_err();
        assert!(format!("{err}").contains("ref"), "got: {err}");
    }
}
