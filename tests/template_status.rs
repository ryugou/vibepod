//! Integration test: `vibepod template status` output when cache absent.

use std::process::Command;

#[test]
fn template_status_reports_missing_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .args(["template", "status"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn vibepod");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("not initialized") || stdout.contains("vibepod init"),
        "expected missing-cache hint, stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
