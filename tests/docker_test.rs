use vibepod::runtime::DockerRuntime;

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
