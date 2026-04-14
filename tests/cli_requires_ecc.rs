//! When `--lang rust` selects an official bundle but no ecc cache exists,
//! vibepod should bail fast with an actionable hint.

use std::process::Command;

fn init_git(dir: &std::path::Path) {
    // Minimal git repo so we get past the "not in git repo" check.
    std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "--allow-empty", "-q", "-m", "init"])
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
}

#[test]
fn lang_rust_without_ecc_cache_errors_with_init_hint() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--lang", "rust", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("vibepod init"),
        "expected init hint, got:\n{combined}"
    );
    assert!(
        combined.contains("rust/impl") || combined.contains("ecc-cache"),
        "expected template name or cache path, got:\n{combined}"
    );
}

#[test]
fn no_lang_no_mode_without_ecc_does_not_require_init() {
    // Host fallback path — no ecc needed, so the init check must NOT fire.
    // We still expect run to fail for other reasons (docker, etc.), but
    // NOT with the ecc init hint.
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The ecc init hint should NOT appear for host-template path.
    assert!(
        !combined.contains("Run `vibepod init` first to clone"),
        "host path must not require ecc init, got:\n{combined}"
    );
}
