use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: serde_json::Value,
}

impl Credentials {
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.claude_ai_oauth.get("expiresAt") {
            if let Some(ts) = expires_at.as_i64() {
                let now = chrono::Utc::now().timestamp_millis();
                return ts < now;
            }
        }
        // If we can't determine expiry, assume not expired
        false
    }
}

pub struct AuthManager {
    config_dir: PathBuf,
    project_dir: PathBuf,
}

impl AuthManager {
    pub fn new(config_dir: PathBuf, project_dir: PathBuf) -> Self {
        Self {
            config_dir,
            project_dir,
        }
    }

    fn shared_dir(&self) -> PathBuf {
        self.config_dir.join("auth")
    }

    fn shared_path(&self) -> PathBuf {
        self.shared_dir().join("credentials.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.shared_dir().join("credentials.lock")
    }

    fn isolated_dir(&self) -> PathBuf {
        self.project_dir
            .join(".vibepod")
            .join("auth")
            .join("containers")
    }

    fn isolated_path(&self, name: &str) -> PathBuf {
        self.isolated_dir().join(format!("{}.json", name))
    }

    pub fn save_shared(&self, creds: &Credentials) -> Result<()> {
        let dir = self.shared_dir();
        fs::create_dir_all(&dir)?;
        let path = self.shared_path();
        let json = serde_json::to_string_pretty(creds)?;
        fs::write(&path, &json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn load_shared(&self) -> Result<Option<Credentials>> {
        let path = self.shared_path();
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let creds: Credentials = serde_json::from_str(&json)?;
        Ok(Some(creds))
    }

    pub fn save_isolated(&self, name: &str, creds: &Credentials) -> Result<()> {
        let dir = self.isolated_dir();
        fs::create_dir_all(&dir)?;
        let path = self.isolated_path(name);
        let json = serde_json::to_string_pretty(creds)?;
        fs::write(&path, &json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn load_isolated(&self, name: &str) -> Result<Option<Credentials>> {
        let path = self.isolated_path(name);
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path)?;
        let creds: Credentials = serde_json::from_str(&json)?;
        Ok(Some(creds))
    }

    pub fn try_acquire_lock(&self, container_name: &str) -> Result<bool> {
        let lock_path = self.lock_path();
        if lock_path.exists() {
            // Check if stale
            if let Ok(existing) = fs::read_to_string(&lock_path) {
                let existing_container = existing.trim();
                if !existing_container.is_empty() && is_container_running(existing_container) {
                    return Ok(false);
                }
            }
            // Stale lock, remove it
            fs::remove_file(&lock_path).ok();
        }
        fs::create_dir_all(self.shared_dir())?;
        fs::write(&lock_path, container_name)?;
        Ok(true)
    }

    pub fn release_lock(&self) -> Result<()> {
        let lock_path = self.lock_path();
        if lock_path.exists() {
            fs::remove_file(&lock_path)?;
        }
        Ok(())
    }

    pub fn lock_info(&self) -> Result<Option<String>> {
        let lock_path = self.lock_path();
        if !lock_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&lock_path)?;
        let name = content.trim().to_string();
        if name.is_empty() {
            Ok(None)
        } else {
            Ok(Some(name))
        }
    }

    pub fn copy_shared_to_temp(&self) -> Result<PathBuf> {
        let creds = self.load_shared()?.context("No shared credentials found")?;
        self.write_creds_to_temp(&creds)
    }

    pub fn copy_isolated_to_temp(&self, name: &str) -> Result<PathBuf> {
        let creds = self
            .load_isolated(name)?
            .context("No isolated credentials found")?;
        self.write_creds_to_temp(&creds)
    }

    fn write_creds_to_temp(&self, creds: &Credentials) -> Result<PathBuf> {
        use std::io::Write;
        let mut temp_file = tempfile::Builder::new()
            .prefix("vibepod-creds-")
            .suffix(".json")
            .tempfile()
            .context("Failed to create temp file")?;
        let json = serde_json::to_string_pretty(creds)?;
        temp_file.write_all(json.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(temp_file.path(), fs::Permissions::from_mode(0o600))?;
        }
        // Persist so it survives the NamedTempFile drop
        let path = temp_file.into_temp_path().keep()?;
        Ok(path)
    }

    pub fn writeback_shared(&self, temp_path: &Path) -> Result<()> {
        let json = fs::read_to_string(temp_path)?;
        let creds: Credentials = serde_json::from_str(&json)?;
        self.save_shared(&creds)
    }

    pub fn writeback_isolated(&self, name: &str, temp_path: &Path) -> Result<()> {
        let json = fs::read_to_string(temp_path)?;
        let creds: Credentials = serde_json::from_str(&json)?;
        self.save_isolated(name, &creds)
    }

    pub fn delete_shared(&self) -> Result<()> {
        let path = self.shared_path();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        self.release_lock()
    }

    pub fn delete_all_isolated(&self) -> Result<()> {
        let dir = self.isolated_dir();
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    pub fn cleanup_temp(&self, temp_path: &Path) {
        fs::remove_file(temp_path).ok();
    }
}

fn is_container_running(container_name: &str) -> bool {
    match std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", container_name])
        .output()
    {
        Ok(o) if o.status.success() => {
            // Only treat as stopped if docker explicitly says "false"
            String::from_utf8_lossy(&o.stdout).trim() != "false"
        }
        // If docker inspect fails (container not found), assume still active
        // to avoid accidentally breaking locks
        _ => true,
    }
}

pub fn detect_oauth_url(text: &str) -> Option<String> {
    let re =
        Regex::new(r#"https://[a-zA-Z0-9._-]*claude[a-zA-Z0-9._-]*/oauth/authorize[^\s)"'>]*"#)
            .unwrap();
    re.find(text).map(|m| m.as_str().to_string())
}

pub fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}
