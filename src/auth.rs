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

/// コンテナ内で claude auth login を実行し、credentials を取得する共通フロー。
/// `-i`（TTY なし）で起動し、stdout をパイプで監視して URL をキャプチャする。
pub fn run_login_flow(image: &str) -> Result<Credentials> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let container_name = format!("vibepod-login-{}", chrono::Utc::now().timestamp_millis());

    // -i (no TTY) so stdout can be piped for URL capture
    let mut child = Command::new("docker")
        .args([
            "run",
            "-i",
            "--network",
            "host",
            "--name",
            &container_name,
            image,
            "claude",
            "auth",
            "login",
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to start login container")?;

    // Monitor stdout for OAuth URL and pass through
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(url) = detect_oauth_url(&line) {
                if !open_browser(&url) {
                    eprintln!("  │  以下の URL をブラウザで開いてください:");
                    eprintln!("  │  {}", url);
                }
            }
            println!("{}", line);
        }
    }

    let status = child.wait()?;

    // Extract credentials via docker cp
    let temp_dir = std::env::temp_dir().join("vibepod-login");
    fs::create_dir_all(&temp_dir)?;
    let temp_creds = temp_dir.join("credentials.json");

    let cp_result = Command::new("docker")
        .args([
            "cp",
            &format!("{}:/home/vibepod/.claude/.credentials.json", container_name),
            &temp_creds.to_string_lossy(),
        ])
        .output();

    // Remove container
    Command::new("docker")
        .args(["rm", "-f", &container_name])
        .output()
        .ok();

    match cp_result {
        Ok(output) if output.status.success() => {
            let json = fs::read_to_string(&temp_creds)?;
            let creds: Credentials = serde_json::from_str(&json)?;
            fs::remove_file(&temp_creds).ok();
            Ok(creds)
        }
        _ => {
            if !status.success() {
                anyhow::bail!("ログインに失敗しました");
            }
            anyhow::bail!("コンテナからクレデンシャルを取得できませんでした")
        }
    }
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
