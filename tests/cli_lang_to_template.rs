//! When `--lang rust` is passed, the rust/impl template should be selected.
//! Verified via VIBEPOD_TRACE=1 stderr output, which precedes downstream
//! (docker/config) failures.

use std::process::Command;

#[test]
fn lang_rust_routes_to_rust_impl() {
    let tmp = tempfile::tempdir().unwrap();
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
