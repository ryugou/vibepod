use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const EXPIRY_THRESHOLD_DAYS: i64 = 7;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    pub token: String,
    pub created_at: String,
    pub expires_at: String,
}

impl TokenData {
    pub fn is_expired(&self) -> bool {
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|exp| chrono::Utc::now() >= exp)
            .unwrap_or(false)
    }

    pub fn needs_renewal(&self) -> bool {
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|exp| {
                let threshold = chrono::Duration::days(EXPIRY_THRESHOLD_DAYS);
                chrono::Utc::now() + threshold >= exp
            })
            .unwrap_or(false)
    }
}

pub struct AuthManager {
    config_dir: PathBuf,
}

impl AuthManager {
    pub fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    fn token_path(&self) -> PathBuf {
        self.config_dir.join("auth").join("token.json")
    }

    pub fn save_token(&self, data: &TokenData) -> Result<()> {
        let dir = self.config_dir.join("auth");
        fs::create_dir_all(&dir)?;
        let path = self.token_path();
        let json = serde_json::to_string_pretty(data)?;
        fs::write(&path, &json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn load_token(&self) -> Result<Option<TokenData>> {
        let path = self.token_path();
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let data: TokenData = serde_json::from_str(&json)?;
        Ok(Some(data))
    }

    pub fn delete_token(&self) -> Result<()> {
        let path = self.token_path();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// コンテナ内で claude setup-token を実行し、長期トークンを取得する。
pub fn run_setup_token(image: &str) -> Result<String> {
    use std::process::Command;

    let container_name = format!("vibepod-login-{}", chrono::Utc::now().timestamp_millis());

    // Create a fake xdg-open script that writes URL to a file for host to pick up
    let url_file = "/tmp/vibepod-oauth-url";
    let fake_open_script = format!("#!/bin/sh\necho \"$1\" > {}\n", url_file);

    // Create temp file for fake xdg-open
    let temp_dir = std::env::temp_dir().join("vibepod-login");
    fs::create_dir_all(&temp_dir)?;
    let fake_open_path = temp_dir.join("xdg-open");
    fs::write(&fake_open_path, &fake_open_script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_open_path, fs::Permissions::from_mode(0o755))?;
    }

    // Start container in background with fake xdg-open mounted
    let run_result = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &container_name,
            "--network",
            "host",
            "-v",
            &format!("{}:/usr/local/bin/xdg-open:ro", fake_open_path.display()),
            image,
            "sleep",
            "300",
        ])
        .output()
        .context("Failed to start login container")?;

    if !run_result.status.success() {
        anyhow::bail!(
            "Failed to start container: {}",
            String::from_utf8_lossy(&run_result.stderr)
        );
    }

    // Start background thread to poll for OAuth URL and open browser on host
    let cn = container_name.clone();
    let url_watcher = std::thread::spawn(move || {
        let poll_file = url_file;
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Ok(output) = Command::new("docker")
                .args(["exec", &cn, "cat", poll_file])
                .output()
            {
                if output.status.success() {
                    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !url.is_empty() {
                        // Open browser on host
                        #[cfg(target_os = "macos")]
                        {
                            Command::new("open").arg(&url).output().ok();
                        }
                        #[cfg(target_os = "linux")]
                        {
                            Command::new("xdg-open").arg(&url).output().ok();
                        }
                        break;
                    }
                }
            }
        }
    });

    // Run claude setup-token via docker exec -it, using script to tee output
    // to a file so we can extract the token afterward
    let token_capture_path = "/tmp/vibepod-setup-token-output";
    let status = Command::new("docker")
        .args([
            "exec",
            "-it",
            &container_name,
            "bash",
            "-c",
            &format!("script -q {} -c 'claude setup-token'", token_capture_path),
        ])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to run claude setup-token")?;

    // Wait for URL watcher to finish (it will timeout if no URL found)
    url_watcher.join().ok();

    // Extract token from captured output
    let output = Command::new("docker")
        .args(["exec", &container_name, "cat", token_capture_path])
        .output()
        .context("Failed to read setup-token output")?;

    // Remove container
    Command::new("docker")
        .args(["rm", "-f", &container_name])
        .output()
        .ok();

    // Clean up temp files
    fs::remove_file(&fake_open_path).ok();

    if !status.success() {
        anyhow::bail!("setup-token に失敗しました");
    }

    // Parse token from output (look for sk-ant-oat01-...)
    let raw_output = String::from_utf8_lossy(&output.stdout);
    let token = raw_output
        .split_whitespace()
        .find(|s| s.starts_with("sk-ant-"))
        .context("トークンが出力から見つかりませんでした")?
        .to_string();

    Ok(token)
}
