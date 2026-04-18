use std::collections::HashMap;
use vibepod::runtime::{ContainerConfig, DockerRuntime};

/// These tests require Docker to be running. Run with:
/// `cargo test --test docker_test -- --ignored`

#[tokio::test]
#[ignore]
async fn test_docker_connection() {
    let runtime = DockerRuntime::new().await;
    assert!(runtime.is_ok(), "Docker should be running for this test");
}

#[tokio::test]
#[ignore]
async fn test_docker_ping() {
    let runtime = DockerRuntime::new().await.unwrap();
    let result = runtime.ping().await;
    assert!(result.is_ok());
}

fn base_config() -> ContainerConfig {
    ContainerConfig {
        image: "test-image:latest".to_string(),
        container_name: "vibepod-test-abc123".to_string(),
        workspace_path: "/tmp/workspace".to_string(),
        claude_json: None,
        gitconfig: None,
        env_vars: vec![],
        network_disabled: false,
        extra_mounts: vec![],
        labels: HashMap::new(),
    }
}

// --- to_create_args tests ---

#[test]
fn test_to_create_args_always_detached() {
    let config = base_config();
    let args = config.to_create_args();
    // コンテナ作成は常に -d（デタッチ）
    assert!(args.contains(&"-d".to_string()));
    assert!(!args.contains(&"-it".to_string()));
    assert!(!args.contains(&"--rm".to_string()));
}

#[test]
fn test_to_create_args_idle_entrypoint() {
    let config = base_config();
    let args = config.to_create_args();
    // 常に idle エントリポイント（tail -f /dev/null）
    assert!(args.contains(&"tail".to_string()));
    assert!(args.contains(&"-f".to_string()));
    assert!(args.contains(&"/dev/null".to_string()));
}

#[test]
fn test_to_create_args_env_vars() {
    let mut config = base_config();
    config.env_vars = vec!["FOO=bar".to_string(), "BAZ=qux".to_string()];
    let args = config.to_create_args();
    // Each env var should be preceded by -e
    let e_positions: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == "-e")
        .map(|(i, _)| i)
        .collect();
    let env_values: Vec<&str> = e_positions
        .iter()
        .filter_map(|&i| args.get(i + 1).map(|s| s.as_str()))
        .collect();
    assert!(env_values.contains(&"FOO=bar"));
    assert!(env_values.contains(&"BAZ=qux"));
}

#[test]
fn test_to_create_args_no_setup_cmd() {
    // セットアップは docker exec で行うため、to_create_args には sh -c が含まれない
    let config = base_config();
    let args = config.to_create_args();
    assert!(!args.contains(&"sh".to_string()));
    assert!(!args.contains(&"-c".to_string()));
}

#[test]
fn test_to_create_args_labels() {
    let mut config = base_config();
    config
        .labels
        .insert("vibepod.lang".to_string(), "rust".to_string());
    let args = config.to_create_args();
    assert!(args.contains(&"--label".to_string()));
    let label_idx = args.iter().position(|a| a == "--label").unwrap();
    assert_eq!(args[label_idx + 1], "vibepod.lang=rust");
}

#[test]
fn test_to_create_args_network_disabled() {
    let mut config = base_config();
    config.network_disabled = true;
    let args = config.to_create_args();
    assert!(args.contains(&"--network".to_string()));
    let net_idx = args.iter().position(|a| a == "--network").unwrap();
    assert_eq!(args[net_idx + 1], "none");
}

// --- vibepod rm prefix validation test ---

#[tokio::test]
async fn test_rm_rejects_non_vibepod_prefix() {
    let result = vibepod::cli::rm::execute(Some("mycontainer".to_string()), false).await;
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("not a VibePod container"));
}

// --- vibepod stop prefix validation test ---

#[tokio::test]
async fn test_stop_rejects_non_vibepod_prefix() {
    let result = vibepod::cli::stop::execute(Some("mycontainer".to_string()), false).await;
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("not a VibePod container"));
}

// --- parse_docker_top_for_claude tests ---

#[test]
fn test_has_claude_process_parses_output() {
    use vibepod::runtime::parse_docker_top_for_claude;

    let output_with_claude = "UID  PID  PPID  CMD\nroot  1  0  tail -f /dev/null\nvibepod  42  1  /home/vibepod/.local/bin/claude --dangerously-skip-permissions -p test\n";
    assert!(parse_docker_top_for_claude(output_with_claude));

    let output_without = "UID  PID  PPID  CMD\nroot  1  0  tail -f /dev/null\n";
    assert!(!parse_docker_top_for_claude(output_without));

    let output_exec =
        "UID  PID  PPID  CMD\nvibepod  10  1  bash --login -c exec claude \"$@\" -- -p test\n";
    assert!(parse_docker_top_for_claude(output_exec));

    // ~/.claude/ パスを含むプロセスは誤検知しない
    let output_claude_dir =
        "UID  PID  PPID  CMD\nvibepod  10  1  cat /home/vibepod/.claude/CLAUDE.md\n";
    assert!(!parse_docker_top_for_claude(output_claude_dir));

    // `docker top -o pid,args` 出力（2 列）でも claude を検出できる
    let output_pid_args_with_claude = "PID  COMMAND\n4388  tail -f /dev/null\n6164  claude --dangerously-skip-permissions -p test\n";
    assert!(parse_docker_top_for_claude(output_pid_args_with_claude));

    // 絶対パス実行の claude も `ends_with("/bin/claude")` で検出できる
    let output_pid_args_abs_path = "PID  COMMAND\n6164  /home/vibepod/.local/bin/claude -p test\n";
    assert!(parse_docker_top_for_claude(output_pid_args_abs_path));

    let output_pid_args_without = "PID  COMMAND\n4388  tail -f /dev/null\n";
    assert!(!parse_docker_top_for_claude(output_pid_args_without));
}

// --- has_claude_process integration tests (require Docker) ---

/// Regression: `docker top -o cmd` fails on Docker Desktop (macOS).
#[tokio::test]
#[ignore]
async fn test_has_claude_process_against_running_container_without_claude() {
    struct ContainerGuard(String);
    impl Drop for ContainerGuard {
        fn drop(&mut self) {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", &self.0])
                .output();
        }
    }

    let runtime = DockerRuntime::new().await.expect("docker runtime");
    // 並列実行・外部同名コンテナとの衝突を避けるため PID を suffix に付ける
    let name = format!(
        "vibepod-test-has-claude-running-idle-{}",
        std::process::id()
    );

    let create = std::process::Command::new("docker")
        .args(["run", "-d", "--name", &name, "alpine", "sleep", "3600"])
        .output()
        .expect("docker run");
    assert!(
        create.status.success(),
        "docker run failed: {}",
        String::from_utf8_lossy(&create.stderr)
    );
    let _guard = ContainerGuard(name.clone());

    let found = runtime
        .has_claude_process(&name)
        .await
        .expect("has_claude_process errored on running container");
    assert!(!found, "idle container must not report claude process");
}
