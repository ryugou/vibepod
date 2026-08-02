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

/// `.vibepod/config.toml` に不正な `profile` を設定した状態で
/// `--worktree --prompt` を実行すると、`git worktree add` /
/// ブランチ作成より前に profile バリデーションで fail-fast すること。
///
/// 順序を誤ると（バリデーションが worktree 作成より後になると）、
/// ユーザーのリポジトリに孤児 worktree（`.worktrees/vibepod-prompt-<ts>`）と
/// 孤児ブランチ（`vibepod/prompt-<ts>`）が残り、`git worktree remove` +
/// `git branch -D` の手動クリーンアップが必要になる。この回帰を検知する。
#[test]
fn test_run_worktree_invalid_profile_fails_before_creating_worktree() {
    let tmp = tempfile::TempDir::new().unwrap();

    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .output()
            .expect("Failed to run git")
    };
    assert!(git(&["init"]).status.success());
    assert!(git(&["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&["config", "user.name", "Test"]).status.success());
    std::fs::write(tmp.path().join("README.md"), "test\n").unwrap();
    assert!(git(&["add", "."]).status.success());
    assert!(git(&["commit", "-m", "initial"]).status.success());

    // `kotlin` は VALID_PROFILES に含まれない不正値。
    std::fs::create_dir_all(tmp.path().join(".vibepod")).unwrap();
    std::fs::write(
        tmp.path().join(".vibepod/config.toml"),
        "[run]\nprofile = \"kotlin\"\n",
    )
    .unwrap();

    let output = Command::new(vibepod_bin())
        .args(["run", "--worktree", "--prompt", "hi"])
        .current_dir(tmp.path())
        .output()
        .expect("Failed to run vibepod");

    assert!(
        !output.status.success(),
        "run must fail fast on invalid profile"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid profile"),
        "Error should mention invalid profile, got: {}",
        stderr
    );

    assert!(
        !tmp.path().join(".worktrees").exists(),
        ".worktrees/ must not be created when profile validation fails first"
    );

    let branch_list = git(&["branch", "--list", "vibepod/prompt-*"]);
    assert!(
        String::from_utf8_lossy(&branch_list.stdout)
            .trim()
            .is_empty(),
        "no vibepod/prompt-* branch should have been created"
    );
}
