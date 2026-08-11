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

/// `list_vibepod_containers` が返すコンテナ情報。
///
/// タプルではなく構造体にしているのは、フィールドが増えたときに呼び出し元が
/// `.0` / `.1` の位置ズレで壊れず、`.name` のように読みやすいまま拡張できるため。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    pub name: String,
    /// docker の `{{.State}}`（running / restarting / paused / exited / created / dead）。
    /// 削除して安全かどうかの判定はこちらを使う（`status` は表示用）。
    pub state: String,
    /// docker の `{{.Status}}`（"Up 5 minutes" 等の表示用文字列）。
    pub status: String,
}

/// コンテナ起動設定。`docker run` に渡す全パラメータを保持する。
/// コンテナは常にアイドルエントリポイント（`tail -f /dev/null`）で起動し、
/// Claude は `docker exec` で実行する。
pub struct ContainerConfig {
    pub image: String,
    pub container_name: String,
    pub workspace_path: String,
    pub claude_json: Option<String>,
    /// `~/.codex/` の allowlist(auth.json / config.toml)をコピーした
    /// per-container ディレクトリの絶対パス。`/home/vibepod/.codex` に
    /// **read-write** でマウントする(codex がトークンリフレッシュ時に
    /// auth.json を書き換えるため。`claude_json` と同じ理由・同じ copy-then-mount
    /// パターン)。`None` の場合はマウントしない(ホストに `~/.codex/auth.json`
    /// が無いケース)。
    pub codex_dir: Option<String>,
    pub gitconfig: Option<String>,
    /// ユーザー環境変数（認証トークンを除く）
    pub env_vars: Vec<String>,
    pub network_disabled: bool,
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

        if let Some(ref codex_dir) = self.codex_dir {
            args.push("-v".to_string());
            args.push(format!("{}:/home/vibepod/.codex", codex_dir));
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

    /// Build the vibepod image.
    ///
    /// `rebuild` maps to `--pull --no-cache`. It exists because the
    /// Dockerfile's `curl … install.sh | bash` layer installs "whatever is
    /// latest right now" but is cached on its literal command text, which
    /// never changes. Without busting the cache, re-running `vibepod init`
    /// replays the old layer and reinstalls the same stale Claude Code —
    /// the user's only recourse was a manual `docker rmi`.
    pub async fn build_image(
        &self,
        dockerfile_content: &str,
        image_name: &str,
        build_args: HashMap<String, String>,
        rebuild: bool,
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

        if rebuild {
            // --no-cache alone would still build on a stale cached base
            // image, so --pull is needed for a genuinely fresh result.
            args.push("--pull".to_string());
            args.push("--no-cache".to_string());
        }

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

    pub async fn list_vibepod_containers(&self) -> Result<Vec<ContainerInfo>> {
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                "name=vibepod-",
                "--format",
                "{{.Names}}\t{{.State}}\t{{.Status}}",
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
            if let Some(info) = parse_vibepod_container_line(line)? {
                result.push(info);
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
    ///
    /// `-o cmd` は Docker Desktop (macOS) の ps バックエンドが拒否するため
    /// `-o pid,args` を使う（`-o args` 単独では PID 列欠落で失敗する）。
    pub async fn has_claude_process(&self, container_name: &str) -> Result<bool> {
        let output = Command::new("docker")
            .args(["top", container_name, "-o", "pid,args"])
            .output()
            .await
            .context("Failed to run docker top")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // コンテナ停止/不存在は期待される失敗 → プロセスなし
            if stderr.contains("No such container") || stderr.contains("is not running") {
                return Ok(false);
            }
            // その他のエラー（権限不足等）は判定不能 → Err にして排他を安全側に倒す
            anyhow::bail!(
                "docker top failed for container {}: {}",
                container_name,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_docker_top_for_claude(&stdout))
    }
}

/// `docker ps --format '{{.Names}}\t{{.State}}\t{{.Status}}'` の1行を
/// `ContainerInfo` にパースする純関数。
///
/// - `Ok(None)`: 空行、または `vibepod-` プレフィックスを持たない行
///   （フィルタ対象外であり、エラーではない）
/// - `Ok(Some(info))`: `vibepod-` プレフィックスを持つ正常な行
/// - `Err`: 空行ではないのにタブ区切りフィールドが3つ揃わない行。
///   これを黙ってスキップすると `list_vibepod_containers` の戻り値が
///   「該当コンテナなし」と区別つかなくなり、`container_removal_decision`
///   （0件を無確認続行と判定する）への入力が fail-open になるため、
///   呼び出し元へ伝播させる。
fn parse_vibepod_container_line(line: &str) -> Result<Option<ContainerInfo>> {
    if line.is_empty() {
        return Ok(None);
    }

    let mut fields = line.splitn(3, '\t');
    let (Some(name), Some(state), Some(status)) = (fields.next(), fields.next(), fields.next())
    else {
        anyhow::bail!(
            "Failed to parse `docker ps` output line: expected 3 tab-separated fields \
             (name, state, status), got {}",
            line.splitn(3, '\t').count()
        );
    };

    if !name.starts_with("vibepod-") {
        return Ok(None);
    }

    Ok(Some(ContainerInfo {
        name: name.to_string(),
        state: state.to_string(),
        status: status.to_string(),
    }))
}

/// `docker top` 出力から claude プロセスを検出する。
/// マッチ条件: コマンドラインのトークンに `claude` が単語として含まれる。
/// `/.claude/` パス（マウントされた設定ディレクトリ）を誤検知しないよう、
/// 単語境界でのみマッチする。
pub fn parse_docker_top_for_claude(output: &str) -> bool {
    for line in output.lines().skip(1) {
        if line
            .split_whitespace()
            .any(|w| w == "claude" || w.ends_with("/bin/claude"))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vibepod_container_line_parses_well_formed_line() {
        let result = parse_vibepod_container_line("vibepod-myproj-abc123\trunning\tUp 5 minutes")
            .unwrap()
            .unwrap();
        assert_eq!(
            result,
            ContainerInfo {
                name: "vibepod-myproj-abc123".to_string(),
                state: "running".to_string(),
                status: "Up 5 minutes".to_string(),
            }
        );
    }

    #[test]
    fn parse_vibepod_container_line_filters_out_non_vibepod_prefix() {
        let result =
            parse_vibepod_container_line("some-other-container\trunning\tUp 5 minutes").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn parse_vibepod_container_line_skips_empty_line() {
        let result = parse_vibepod_container_line("").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn parse_vibepod_container_line_errors_on_missing_fields() {
        // 混入検出用に、他のテストでは使わない識別しやすいコンテナ名を使う。
        let line = "vibepod-secretproject-abc123\trunning";
        let err = parse_vibepod_container_line(line).unwrap_err();
        let message = err.to_string();

        // 診断情報としてフィールド数などの構造情報は含んでよい。
        assert!(
            message.contains("expected 3 tab-separated fields") && message.contains("got 2"),
            "error message should describe the field-count mismatch, got: {:?}",
            message
        );
        // 将来「入力行を埋め込んで診断を改善する」変更が入っても、行内容や
        // コンテナ名（他プロジェクトの情報を含みうる）を漏らさないことを固定する。
        assert!(
            !message.contains(line),
            "error message must not embed the raw input line, got: {:?}",
            message
        );
        assert!(
            !message.contains("vibepod-secretproject-abc123"),
            "error message must not embed the container name, got: {:?}",
            message
        );
    }
}
