use anyhow::{Context, Result};
use regex::Regex;
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

    // Start container in background
    let run_result = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &container_name,
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

    // Run claude setup-token via docker exec with script for PTY + stdout capture
    let mut child = Command::new("docker")
        .args([
            "exec",
            "-i",
            &container_name,
            "script",
            "-q",
            "/dev/null",
            "-c",
            "COLUMNS=10000 claude setup-token",
        ])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .context("Failed to run claude setup-token")?;

    // Monitor stdout for OAuth URL, open browser, and relay output
    // URL may be split across multiple lines due to terminal wrapping,
    // so we accumulate cleaned text and check for URL across line boundaries.
    let mut url_opened = false;
    let mut accumulated_clean = String::new();
    if let Some(stdout) = child.stdout.take() {
        use std::io::{BufRead, BufReader, Write};
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            // Relay original output to terminal
            let _ = std::io::stdout().write_all(line.as_bytes());
            let _ = std::io::stdout().write_all(b"\n");
            let _ = std::io::stdout().flush();

            if !url_opened {
                let clean = strip_ansi_codes(&line);
                accumulated_clean.push_str(&clean);
                if let Some(url) = detect_oauth_url(&accumulated_clean) {
                    open_browser(&url);
                    url_opened = true;
                }
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        Command::new("docker")
            .args(["rm", "-f", &container_name])
            .output()
            .ok();
        anyhow::bail!("setup-token に失敗しました");
    }

    // Extract credentials from container
    let output = Command::new("docker")
        .args([
            "exec",
            &container_name,
            "cat",
            "/home/vibepod/.claude/.credentials.json",
        ])
        .output()
        .context("Failed to read credentials from container")?;

    // Remove container
    Command::new("docker")
        .args(["rm", "-f", &container_name])
        .output()
        .ok();

    if !output.status.success() {
        anyhow::bail!("コンテナからトークンを取得できませんでした");
    }

    // Parse the token from credentials
    let json_str = String::from_utf8_lossy(&output.stdout);
    let creds: serde_json::Value =
        serde_json::from_str(&json_str).context("credentials.json のパースに失敗しました")?;

    let token = creds
        .get("claudeAiOauth")
        .and_then(|o| o.get("accessToken"))
        .and_then(|t| t.as_str())
        .context("accessToken が見つかりません")?
        .to_string();

    Ok(token)
}

fn detect_oauth_url(text: &str) -> Option<String> {
    // Require &state= parameter to ensure we have the complete URL
    let re = Regex::new(
        r#"https://[a-zA-Z0-9._-]*claude[a-zA-Z0-9._-]*/oauth/authorize[^\s)"'>]*&state=[a-zA-Z0-9_-]+"#,
    )
    .unwrap();
    re.find(text).map(|m| m.as_str().to_string())
}

fn strip_ansi_codes(text: &str) -> String {
    let re = Regex::new(concat!(
        r"\x1b\[[0-9;?]*[a-zA-Z]",        // CSI sequences
        r"|\x1b\][^\x07]*\x07",           // OSC sequences (BEL terminated)
        r"|\x1bP[^\x1b]*\x1b\\",          // DCS sequences
        r"|\x1b[a-zA-Z]",                 // Two-char ESC sequences
        r"|[\x00-\x08\x0b\x0c\x0e-\x1f]", // Other control chars (except \t \n \r)
    ))
    .unwrap();
    re.replace_all(text, "").to_string()
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).output().ok();
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .output()
            .ok();
    }
}
