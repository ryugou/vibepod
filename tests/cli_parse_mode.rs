//! Verify `--mode` parses correctly (accepted values + rejection of bogus).

use std::process::Command;

#[test]
fn mode_review_is_accepted() {
    // Run fails downstream (no Docker / no ecc), but --mode must parse cleanly.
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .args(["run", "--mode", "review", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("unrecognized") && !combined.contains("unexpected argument"),
        "flag should parse; got: {combined}"
    );
}

#[test]
fn mode_impl_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .args(["run", "--mode", "impl", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("unrecognized") && !combined.contains("unexpected argument"),
        "flag should parse; got: {combined}"
    );
}

#[test]
fn mode_bogus_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .args(["run", "--mode", "bogus", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "bogus mode should fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("bogus") || combined.contains("invalid value"),
        "expected bogus-value rejection, got: {combined}"
    );
}
