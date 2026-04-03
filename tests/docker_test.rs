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
        args: vec!["claude".to_string()],
        env_vars: vec![],
        network_disabled: false,
        setup_cmd: None,
        codex_auth: None,
        extra_mounts: vec![],
        reuse: false,
        reuse_entrypoint: false,
    }
}

// --- to_docker_args tests ---

#[test]
fn test_to_docker_args_interactive() {
    let config = base_config();
    let args = config.to_docker_args(true);
    assert!(args.contains(&"-it".to_string()));
    assert!(args.contains(&"--rm".to_string()));
    assert!(!args.contains(&"-d".to_string()));
}

#[test]
fn test_to_docker_args_detached() {
    let config = base_config();
    let args = config.to_docker_args(false);
    assert!(args.contains(&"-d".to_string()));
    assert!(!args.contains(&"-it".to_string()));
}

#[test]
fn test_to_docker_args_reuse() {
    let mut config = base_config();
    config.reuse = true;
    config.container_name = "vibepod-myproject-reuse".to_string();
    let args = config.to_docker_args(true);
    // --rm must not be present in reuse mode
    assert!(!args.contains(&"--rm".to_string()));
    assert!(config.container_name.contains("-reuse"));
}

#[test]
fn test_to_docker_args_env_vars() {
    let mut config = base_config();
    config.env_vars = vec!["FOO=bar".to_string(), "BAZ=qux".to_string()];
    let args = config.to_docker_args(false);
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
fn test_to_docker_args_setup_cmd() {
    let mut config = base_config();
    config.setup_cmd = Some("apt-get install -y curl".to_string());
    let args = config.to_docker_args(false);
    // setup_cmd wraps with sh -c
    assert!(args.contains(&"sh".to_string()));
    assert!(args.contains(&"-c".to_string()));
    let sh_c_idx = args.iter().position(|a| a == "-c").unwrap();
    let cmd_str = &args[sh_c_idx + 1];
    assert!(cmd_str.contains("apt-get install -y curl"));
}

// --- vibepod rm prefix validation test ---

#[tokio::test]
async fn test_rm_rejects_non_vibepod_prefix() {
    let result = vibepod::cli::rm::execute(Some("mycontainer".to_string()), false).await;
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("not a VibePod container"));
}
