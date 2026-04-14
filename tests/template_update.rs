//! Integration test: `vibepod template update` without a cloned cache
//! should exit non-zero and point the user at `vibepod init`.

use std::process::Command;

#[test]
fn template_update_requires_init() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .args(["template", "update"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn vibepod");
    assert!(
        !out.status.success(),
        "expected non-zero exit on missing cache"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("vibepod init"),
        "expected init hint in output, got:\n{combined}"
    );
}
