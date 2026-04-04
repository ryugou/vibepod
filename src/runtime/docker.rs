use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::process::Command;

/// コンテナの状態を表す列挙型。
#[derive(Debug, Clone, PartialEq)]
pub enum ContainerStatus {
    /// コンテナが存在しない
    None,
    /// コンテナが停止中（exited）
    Stopped,
    /// コンテナが実行中（running）
    Running,
}

/// Docker CLI ラッパー。docker コマンドを通じてコンテナ操作を行う。
pub struct DockerRuntime;

/// コンテナ起動設定。`docker run` に渡す全パラメータを保持する。
/// コンテナは常にアイドルエントリポイント（`tail -f /dev/null`）で起動し、
/// Claude は `docker exec` で実行する。
pub struct ContainerConfig {
    pub image: String,
    pub container_name: String,
    pub workspace_path: String,
    pub claude_json: Option<String>,
    pub gitconfig: Option<String>,
    /// ユーザー環境変数（認証トークンを除く）
    pub env_vars: Vec<String>,
    pub network_disabled: bool,
    pub codex_auth: Option<String>,
    pub extra_mounts: Vec<(String, String)>,
    /// コンテナ作成時に付与するラベル（設定変更の検知に使用）
    pub labels: HashMap<String, String>,
}

impl ContainerConfig {
    /// `docker run -d` 用の引数を生成する。
    /// コンテナは常にアイドルエントリポイント（`tail -f /dev/null`）で起動する。
    pub fn to_create_args(&self) -> Vec<String> {
        let mut args = vec!["run".to_string(), "-d".to_string()];
        args.push("--name".to_string());
        args.push(self.container_name.clone());
        args.push("-v".to_string());
        args.push(format!("{}:/workspace", self.workspace_path));

        if let Some(ref gitconfig) = self.gitconfig {
            args.push("-v".to_string());
            args.push(format!("{}:/home/vibepod/.gitconfig:ro", gitconfig));
        }

        for (host, container) in &self.extra_mounts {
            args.push("-v".to_string());
            args.push(format!("{}:{}:ro", host, container));
        }

        if let Some(ref claude_json) = self.claude_json {
            args.push("-v".to_string());
            args.push(format!("{}:/home/vibepod/.claude.json", claude_json));
        }

        if let Some(ref codex_auth) = self.codex_auth {
            args.push("-v".to_string());
            args.push(format!("{}:/home/vibepod/.codex/auth.json:ro", codex_auth));
        }

        if self.network_disabled {
            args.push("--network".to_string());
            args.push("none".to_string());
        }

        for env_var in &self.env_vars {
            args.push("-e".to_string());
            args.push(env_var.clone());
        }
        args.push("-e".to_string());
        args.push("TERM=xterm-256color".to_string());

        for (key, value) in &self.labels {
            args.push("--label".to_string());
            args.push(format!("{}={}", key, value));
        }

        args.push(self.image.clone());

        // 常にアイドルエントリポイントで起動
        args.push("tail".to_string());
        args.push("-f".to_string());
        args.push("/dev/null".to_string());

        args
    }
}

impl DockerRuntime {
    pub async fn new() -> Result<Self> {
        Ok(Self)
    }

    pub async fn ping(&self) -> Result<()> {
        let output = Command::new("docker")
            .args(["info"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .await
            .context("Failed to run docker info. Is Docker Desktop or OrbStack running?")?;
        if !output.status.success() {
            anyhow::bail!("Docker is not responding. Is Docker Desktop or OrbStack running?");
        }
        Ok(())
    }

    pub async fn build_image(
        &self,
        dockerfile_content: &str,
        image_name: &str,
        build_args: HashMap<String, String>,
    ) -> Result<()> {
        use std::io::Write as IoWrite;

        let temp_dir = tempfile::tempdir().context("Failed to create temporary build directory")?;
        let dockerfile_path = temp_dir.path().join("Dockerfile");
        let mut file = std::fs::File::create(&dockerfile_path)?;
        file.write_all(dockerfile_content.as_bytes())?;

        let mut args = vec![
            "build".to_string(),
            "-f".to_string(),
            dockerfile_path.to_string_lossy().to_string(),
            "-t".to_string(),
            image_name.to_string(),
        ];

        for (k, v) in &build_args {
            args.push("--build-arg".to_string());
            args.push(format!("{}={}", k, v));
        }

        args.push(temp_dir.path().to_string_lossy().to_string());

        let status = Command::new("docker")
            .args(&args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("Failed to run docker build")?;

        if !status.success() {
            anyhow::bail!("docker build failed");
        }

        Ok(())
    }

    pub async fn image_exists(&self, image_name: &str) -> Result<bool> {
        let output = Command::new("docker")
            .args(["inspect", "--type", "image", image_name])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to run docker inspect")?;

        if output.status.success() {
            return Ok(true);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such image") || stderr.contains("No such object") {
            Ok(false)
        } else {
            anyhow::bail!("docker inspect failed: {}", stderr.trim())
        }
    }

    /// コンテナの状態（None / Stopped / Running）を返す。
    pub async fn find_container_status(&self, name: &str) -> Result<ContainerStatus> {
        let filter = format!("name={}", name);
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &filter,
                "--format",
                "{{.Names}}\t{{.Status}}",
            ])
            .output()
            .await
            .context("Failed to run docker ps")?;

        if !output.status.success() {
            anyhow::bail!(
                "docker ps failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some((container_name, status)) = line.split_once('\t') {
                if container_name == name {
                    if status.starts_with("Up") || status.to_lowercase().contains("running") {
                        return Ok(ContainerStatus::Running);
                    } else {
                        return Ok(ContainerStatus::Stopped);
                    }
                }
            }
        }
        Ok(ContainerStatus::None)
    }

    /// コンテナのラベルを取得する。
    pub async fn get_container_labels(&self, name: &str) -> Result<HashMap<String, String>> {
        let output = Command::new("docker")
            .args(["inspect", "--format", "{{json .Config.Labels}}", name])
            .output()
            .await
            .context("Failed to run docker inspect")?;

        if !output.status.success() {
            return Ok(HashMap::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout == "null" || stdout.is_empty() {
            return Ok(HashMap::new());
        }

        let labels: HashMap<String, String> = serde_json::from_str(&stdout).unwrap_or_default();
        Ok(labels)
    }

    /// `/home/vibepod/.vibepod-setup-done` マーカーファイルの存在を確認する。
    pub async fn check_setup_marker(&self, name: &str) -> Result<bool> {
        let output = Command::new("docker")
            .args([
                "exec",
                name,
                "test",
                "-f",
                "/home/vibepod/.vibepod-setup-done",
            ])
            .output()
            .await
            .context("Failed to run docker exec test")?;
        Ok(output.status.success())
    }

    pub async fn find_running_container(
        &self,
        name_prefix: &str,
    ) -> Result<Option<(String, String)>> {
        let filter = format!("name={}-", name_prefix);
        let output = Command::new("docker")
            .args(["ps", "--filter", &filter, "--format", "{{.ID}}\t{{.Names}}"])
            .output()
            .await
            .context("Failed to run docker ps")?;

        if !output.status.success() {
            anyhow::bail!(
                "docker ps failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let prefix_with_dash = format!("{}-", name_prefix);
        for line in stdout.lines() {
            if let Some((id, name)) = line.split_once('\t') {
                if name.starts_with(&prefix_with_dash) {
                    return Ok(Some((id.to_string(), name.to_string())));
                }
            }
        }
        Ok(None)
    }

    /// Find a container by exact name that is in the exited (stopped) state.
    pub async fn find_stopped_container(&self, name: &str) -> Result<Option<String>> {
        let filter_name = format!("name={}", name);
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &filter_name,
                "--filter",
                "status=exited",
                "--format",
                "{{.ID}}\t{{.Names}}",
            ])
            .output()
            .await
            .context("Failed to run docker ps")?;

        if !output.status.success() {
            anyhow::bail!(
                "docker ps failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some((id, container_name)) = line.split_once('\t') {
                if container_name == name {
                    return Ok(Some(id.to_string()));
                }
            }
        }
        Ok(None)
    }

    pub async fn list_vibepod_containers(&self) -> Result<Vec<(String, String)>> {
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                "name=vibepod-",
                "--format",
                "{{.Names}}\t{{.Status}}",
            ])
            .output()
            .await
            .context("Failed to run docker ps")?;

        if !output.status.success() {
            anyhow::bail!(
                "docker ps failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut result = Vec::new();
        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }
            if let Some((name, status)) = line.split_once('\t') {
                if name.starts_with("vibepod-") {
                    result.push((name.to_string(), status.to_string()));
                }
            }
        }
        Ok(result)
    }

    pub async fn find_container_by_name(&self, name: &str) -> Result<Option<String>> {
        let filter = format!("name={}", name);
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &filter,
                "--format",
                "{{.ID}}\t{{.Names}}",
            ])
            .output()
            .await
            .context("Failed to run docker ps")?;

        if !output.status.success() {
            anyhow::bail!(
                "docker ps failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some((id, container_name)) = line.split_once('\t') {
                if container_name == name {
                    return Ok(Some(id.to_string()));
                }
            }
        }
        Ok(None)
    }

    pub async fn get_logs(&self, container_id: &str, tail: &str) -> Result<()> {
        let status = Command::new("docker")
            .args(["logs", "--tail", tail, container_id])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("Failed to run docker logs")?;
        if !status.success() {
            anyhow::bail!("docker logs failed for container {}", container_id);
        }
        Ok(())
    }

    pub async fn stream_logs(&self, container_id: &str) -> Result<()> {
        let status = Command::new("docker")
            .args(["logs", "--follow", container_id])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("Failed to run docker logs")?;
        if !status.success() {
            anyhow::bail!("docker logs failed for container {}", container_id);
        }
        Ok(())
    }

    pub async fn start_container(&self, container_id: &str) -> Result<()> {
        let status = Command::new("docker")
            .args(["start", container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
            .context("Failed to run docker start")?;
        if !status.success() {
            anyhow::bail!("docker start failed for container {}", container_id);
        }
        Ok(())
    }

    pub async fn stop_container(&self, container_id: &str, timeout_secs: u32) -> Result<()> {
        let timeout_str = timeout_secs.to_string();
        let status = Command::new("docker")
            .args(["stop", "-t", &timeout_str, container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
            .context("Failed to run docker stop")?;
        if !status.success() {
            anyhow::bail!("docker stop failed for container {}", container_id);
        }
        Ok(())
    }

    pub async fn remove_container(&self, container_id: &str) -> Result<()> {
        let status = Command::new("docker")
            .args(["rm", "-f", container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
            .context("Failed to run docker rm")?;
        if !status.success() {
            anyhow::bail!("docker rm failed for container {}", container_id);
        }
        Ok(())
    }

    /// コンテナ内で claude プロセスが実行中かどうかを確認する。
    pub async fn has_claude_process(&self, container_name: &str) -> Result<bool> {
        let output = Command::new("docker")
            .args(["top", container_name, "-o", "cmd"])
            .output()
            .await
            .context("Failed to run docker top")?;

        if !output.status.success() {
            return Ok(false);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_docker_top_for_claude(&stdout))
    }
}

/// `docker top` 出力から claude プロセスを検出する。
pub fn parse_docker_top_for_claude(output: &str) -> bool {
    for line in output.lines().skip(1) {
        let line_lower = line.to_lowercase();
        if line_lower.contains("/claude") || line_lower.split_whitespace().any(|w| w == "claude") {
            return true;
        }
    }
    false
}
