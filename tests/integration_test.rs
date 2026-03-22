use std::path::PathBuf;
use std::process::Command;

/// Get the path to the built binary
fn vibepod_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vibepod"))
}

#[test]
fn test_vibepod_version() {
    let output = Command::new(vibepod_bin())
        .arg("--version")
        .output()
        .expect("Failed to run vibepod");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("vibepod"));
}

#[test]
fn test_run_interactive_outside_git_repo_fails() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = Command::new(vibepod_bin())
        .arg("run")
        .current_dir(tmp.path())
        .output()
        .expect("Failed to run vibepod");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git") || stderr.contains("repository"),
        "Error should mention git repository, got: {}",
        stderr
    );
}

#[test]
fn test_run_outside_git_repo_fails() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = Command::new(vibepod_bin())
        .args(["run", "--resume"])
        .current_dir(tmp.path())
        .output()
        .expect("Failed to run vibepod");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git") || stderr.contains("repository"),
        "Error should mention git repository, got: {}",
        stderr
    );
}
