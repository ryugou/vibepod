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
        codex_auth: None,
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
