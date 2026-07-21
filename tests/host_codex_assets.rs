//! Host `~/.codex/` assets copied into the container for codex review support.
//!
//! The invariant under test: only an allowlisted subset of `~/.codex/`
//! (`auth.json` / `config.toml`) is ever copied into the per-container
//! runtime directory that gets mounted as `/home/vibepod/.codex`. Session
//! history (`history.jsonl`), goal databases (`goals_*.sqlite`), and any
//! `cache/` contents must never travel in — they are sensitive and
//! unnecessary for running `codex`.

use std::fs;
use std::path::Path;

use vibepod::cli::run::{host_codex_stage_entries, prepare_codex_mount};

/// Build a host `~/.codex/` containing both allowlisted assets and the
/// history/cache data that must be left behind.
fn make_host_codex(home: &Path) {
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();

    // Allowlisted
    fs::write(codex.join("auth.json"), r#"{"token":"HOST_AUTH"}"#).unwrap();
    fs::write(codex.join("config.toml"), "model = \"gpt\"\n").unwrap();

    // Must never be carried into the container
    fs::write(codex.join("history.jsonl"), "SECRET_HISTORY").unwrap();
    fs::write(codex.join("goals_x.sqlite"), "SECRET_GOALS").unwrap();
    fs::create_dir_all(codex.join("cache")).unwrap();
    fs::write(codex.join("cache/entry"), "SECRET_CACHE").unwrap();
}

// --- host_codex_stage_entries (allowlist) ---

#[test]
fn stage_entries_selects_only_allowlisted_files() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    make_host_codex(&home);

    let names: Vec<&str> = host_codex_stage_entries(&home.join(".codex"))
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert_eq!(names, vec!["auth.json", "config.toml"]);
}

#[test]
fn stage_entries_skips_absent_files_without_error() {
    // A host with only auth.json (no config.toml) is a normal setup, not an
    // error: config.toml omission means "use codex's own defaults".
    let tmp = tempfile::tempdir().unwrap();
    let codex = tmp.path().join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(codex.join("auth.json"), r#"{"token":"only-auth"}"#).unwrap();

    let names: Vec<&str> = host_codex_stage_entries(&codex)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert_eq!(names, vec!["auth.json"]);
}

// --- prepare_codex_mount ---

#[test]
fn prepare_codex_mount_returns_none_when_auth_json_missing() {
    // No ~/.codex/ at all.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    let result =
        prepare_codex_mount(home_dir.path(), config_dir.path(), "vibepod-test-none").unwrap();

    assert!(
        result.is_none(),
        "should return None when ~/.codex/auth.json is absent"
    );
}

#[test]
fn prepare_codex_mount_returns_none_when_only_config_toml_present() {
    // config.toml without auth.json: codex review cannot authenticate, so no
    // mount should be prepared (auth.json is the load-bearing file).
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let codex_dir = home_dir.path().join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(codex_dir.join("config.toml"), "model = \"gpt\"\n").unwrap();

    let result =
        prepare_codex_mount(home_dir.path(), config_dir.path(), "vibepod-test-cfg-only").unwrap();

    assert!(
        result.is_none(),
        "should return None when auth.json is absent even if config.toml exists"
    );
}

#[test]
fn prepare_codex_mount_copies_both_files_and_returns_dir() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let result = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-both-files",
    )
    .unwrap();

    let dir = result.expect("should return Some(dir) when auth.json is present");

    let copied_auth = dir.join("auth.json");
    let copied_config = dir.join("config.toml");
    assert!(copied_auth.is_file(), "auth.json should be copied");
    assert!(copied_config.is_file(), "config.toml should be copied");

    assert_eq!(
        fs::read_to_string(&copied_auth).unwrap(),
        r#"{"token":"HOST_AUTH"}"#
    );
    assert_eq!(
        fs::read_to_string(&copied_config).unwrap(),
        "model = \"gpt\"\n"
    );

    // history.jsonl / goals_*.sqlite / cache/ must never be copied alongside.
    assert!(!dir.join("history.jsonl").exists());
    assert!(!dir.join("goals_x.sqlite").exists());
    assert!(!dir.join("cache").exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "runtime codex dir should be 0700, got {:o}",
            dir_mode
        );

        let auth_mode = fs::metadata(&copied_auth).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            auth_mode, 0o600,
            "auth.json copy should be 0600, got {:o}",
            auth_mode
        );

        let config_mode = fs::metadata(&copied_config).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            config_mode, 0o600,
            "config.toml copy should be 0600, got {:o}",
            config_mode
        );
    }
}
