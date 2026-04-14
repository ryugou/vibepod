//! Reject `--template <name> --mode review` as incompatible.

use std::process::Command;

#[test]
fn template_plus_mode_review_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .args([
            "run",
            "--template",
            "my-custom",
            "--mode",
            "review",
            "--prompt",
            "x",
        ])
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
        combined.contains("--mode") && combined.contains("--template"),
        "error must name both flags; got:\n{combined}"
    );
}

#[test]
fn template_plus_mode_impl_still_allowed() {
    // `--mode impl` is default and should coexist with `--template`.
    // The run will fail later for other reasons (no Docker in tests),
    // but NOT from the template+mode conflict check.
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .args([
            "run",
            "--template",
            "my-custom",
            "--mode",
            "impl",
            "--prompt",
            "x",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The conflict check should NOT trigger:
    assert!(
        !(combined.contains("--mode")
            && combined.contains("--template")
            && combined.contains("cannot")),
        "template+mode-impl must not trigger the conflict error; got:\n{combined}"
    );
}
