//! When `--lang rust` is passed, the rust/impl template should be selected.
//! Verified via VIBEPOD_TRACE=1 stderr output, which precedes downstream
//! (docker/config) failures.

use std::path::Path;
use std::process::Command;

/// Initialize a minimal git repo in `dir` so `prepare_context`'s
/// `is_git_repo` check passes and execution proceeds to the template
/// resolver (which is what we're testing). The trace is emitted *after*
/// the final `or_else` resolution step, so downstream steps like git /
/// docker must not short-circuit the run before that point.
fn init_git(dir: &Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            // Ensure user.name/user.email are defined for the commit below
            // without depending on the host's global git config.
            .env("GIT_AUTHOR_NAME", "vibepod-test")
            .env("GIT_AUTHOR_EMAIL", "test@vibepod.invalid")
            .env("GIT_COMMITTER_NAME", "vibepod-test")
            .env("GIT_COMMITTER_EMAIL", "test@vibepod.invalid")
            .status()
            .expect("git");
        assert!(
            status.success(),
            "git {:?} failed in {}",
            args,
            dir.display()
        );
    };
    run(&["init", "-q", "-b", "main"]);
    // An initial commit is required because prepare_context reads HEAD.
    run(&["commit", "--allow-empty", "-q", "-m", "init"]);
}

#[test]
fn lang_rust_routes_to_rust_impl() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--lang", "rust", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("selected template = rust/impl"),
        "expected rust/impl selection, stderr:\n{stderr}"
    );
}

#[test]
fn lang_and_mode_review_routes_to_rust_review() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--lang", "rust", "--mode", "review", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("selected template = rust/review"),
        "expected rust/review, stderr:\n{stderr}"
    );
}

#[test]
fn mode_review_without_lang_routes_to_generic_review() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--mode", "review", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("selected template = generic/review"),
        "expected generic/review, stderr:\n{stderr}"
    );
}

#[test]
fn no_lang_no_mode_routes_to_host() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // `(None, Impl)` falls through to host; no cwd-detect will fire in a
    // tempdir that has no recognized project files.
    assert!(
        stderr.contains("selected template = <host>"),
        "expected host fallback, stderr:\n{stderr}"
    );
}

#[test]
fn explicit_lang_wins_over_cwd_detect() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    // Write a Cargo.toml so cwd-detect would pick rust,
    // but pass --lang go to prove explicit flag takes priority.
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"fake\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--lang", "go", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("selected template = go/impl"),
        "explicit --lang go must win over cwd Cargo.toml, stderr:\n{stderr}"
    );
}
