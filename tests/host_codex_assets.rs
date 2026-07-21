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
use std::time::{Duration, SystemTime};

use vibepod::cli::run::{host_codex_stage_entries, prepare_codex_mount, should_keep_staged_auth};

/// Set a file's mtime deterministically, without relying on real-time sleeps
/// (which would make the mtime-ordering tests flaky). Stable since Rust 1.75.
fn set_mtime(path: &Path, when: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap_or_else(|e| panic!("failed to open {} for mtime write: {e}", path.display()));
    let times = fs::FileTimes::new().set_modified(when);
    file.set_times(times)
        .unwrap_or_else(|e| panic!("failed to set mtime on {}: {e}", path.display()));
}

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

// --- prepare_codex_mount: reconciliation of stale staged files (codex review round 1) ---

#[test]
fn prepare_codex_mount_removes_staged_files_when_auth_json_disappears() {
    // First run stages both files into the runtime dir.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-auth-disappears",
    )
    .unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    assert!(dir.join("auth.json").is_file());
    assert!(dir.join("config.toml").is_file());

    // Host revokes auth (e.g. `codex logout`): auth.json is gone.
    fs::remove_file(home_dir.path().join(".codex/auth.json")).unwrap();

    let second = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-auth-disappears",
    )
    .unwrap();

    assert!(
        second.is_none(),
        "should return None once host auth.json is gone, even though it was staged before"
    );
    assert!(
        !dir.join("auth.json").exists(),
        "stale staged auth.json must be removed so a bind-mounted container can't keep using it"
    );
    assert!(
        !dir.join("config.toml").exists(),
        "stale staged config.toml must be removed alongside auth.json on revocation"
    );
    assert!(
        dir.is_dir(),
        "the staging directory itself must survive so an existing bind mount's inode stays valid"
    );
}

#[test]
fn prepare_codex_mount_reconciles_config_toml_removal_and_refreshes_auth() {
    // First run stages both files into the runtime dir.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-config-removed",
    )
    .unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    assert!(dir.join("config.toml").is_file());

    // Host drops config.toml (falls back to codex defaults) and rotates auth.json.
    fs::remove_file(home_dir.path().join(".codex/config.toml")).unwrap();
    fs::write(
        home_dir.path().join(".codex/auth.json"),
        r#"{"token":"ROTATED_AUTH"}"#,
    )
    .unwrap();

    let second = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-config-removed",
    )
    .unwrap();

    let second_dir = second.expect("auth.json is still present, so a mount should be prepared");
    assert_eq!(second_dir, dir);
    assert!(
        !second_dir.join("config.toml").exists(),
        "stale staged config.toml must be removed once the host copy disappears"
    );
    assert_eq!(
        fs::read_to_string(second_dir.join("auth.json")).unwrap(),
        r#"{"token":"ROTATED_AUTH"}"#,
        "staged auth.json must be refreshed with the host's rotated content"
    );
    assert!(
        second_dir.is_dir(),
        "the staging directory itself must survive reconciliation"
    );
}

// --- should_keep_staged_auth (pure mtime comparison, codex review round 3) ---

#[test]
fn should_keep_staged_auth_true_when_staged_is_newer() {
    let host = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let staged = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
    assert!(should_keep_staged_auth(host, staged));
}

#[test]
fn should_keep_staged_auth_false_when_host_is_newer() {
    let host = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
    let staged = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(!should_keep_staged_auth(host, staged));
}

#[test]
fn should_keep_staged_auth_false_when_mtimes_are_equal() {
    // Equal mtimes must fall through to "host wins" (overwrite), matching the
    // pre-round-3 behavior when no reliable ordering can be established.
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(!should_keep_staged_auth(t, t));
}

// --- prepare_codex_mount: keep-newer-auth (codex review round 3, P1) ---

#[test]
fn prepare_codex_mount_keeps_staged_auth_when_newer_than_host() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-keep-newer-staged-auth",
    )
    .unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");

    // Simulate the in-container codex rotating its refresh token: the staged
    // (rw-mounted) auth.json is rewritten and now carries a newer mtime than
    // the host's original, untouched copy.
    fs::write(&staged_auth, r#"{"token":"CONTAINER_REFRESHED"}"#).unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    let second = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-keep-newer-staged-auth",
    )
    .unwrap();

    assert_eq!(second, Some(dir));
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED"}"#,
        "the container's rotated auth.json must survive a subsequent `vibepod run`, \
         not be clobbered by the stale host copy"
    );
}

#[test]
fn prepare_codex_mount_overwrites_staged_auth_when_host_is_newer() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-host-newer-auth",
    )
    .unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    set_mtime(&staged_auth, t0);

    // User re-authenticates on the host (e.g. `codex login`): the host copy
    // is rewritten with a strictly newer mtime than the staged copy.
    fs::write(&host_auth, r#"{"token":"HOST_RELOGIN"}"#).unwrap();
    set_mtime(&host_auth, t0 + Duration::from_secs(3_600));

    let second = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-host-newer-auth",
    )
    .unwrap();

    assert_eq!(second, Some(dir));
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"HOST_RELOGIN"}"#,
        "a newer host auth.json (e.g. after re-login) must still overwrite the staged copy"
    );
}

#[test]
fn prepare_codex_mount_always_overwrites_config_toml_even_if_staged_is_newer() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_config = home_dir.path().join(".codex/config.toml");

    let t0 = SystemTime::now();
    set_mtime(&host_config, t0);

    let first = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-config-toml-no-keep-newer",
    )
    .unwrap();
    let dir = first.expect("first run should stage config.toml");
    let staged_config = dir.join("config.toml");

    // Give the staged config.toml a much newer mtime than the host's, mirroring
    // the auth.json "keep newer" scenario above. Unlike auth.json, config.toml
    // is not a credential the container mutates, so it must always lose to the
    // host copy regardless of mtime ordering.
    fs::write(&staged_config, "STAGED_ONLY_SHOULD_NOT_SURVIVE\n").unwrap();
    set_mtime(&staged_config, t0 + Duration::from_secs(3_600));

    fs::write(&host_config, "model = \"host-updated\"\n").unwrap();
    set_mtime(&host_config, t0 + Duration::from_secs(60));

    let second = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-config-toml-no-keep-newer",
    )
    .unwrap();

    assert_eq!(second, Some(dir));
    assert_eq!(
        fs::read_to_string(&staged_config).unwrap(),
        "model = \"host-updated\"\n",
        "config.toml has no keep-newer exemption: the host copy must always win"
    );
}

#[test]
fn prepare_codex_mount_auth_removal_ignores_staged_mtime() {
    // Regression guard (round 1 semantics): even if the staged auth.json looks
    // "newer" than the host's last-known auth.json, deleting the host file is
    // an explicit revocation and must always win over any mtime comparison.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-auth-removal-ignores-mtime",
    )
    .unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    let staged_auth = dir.join("auth.json");
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    fs::remove_file(&host_auth).unwrap();

    let second = prepare_codex_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-auth-removal-ignores-mtime",
    )
    .unwrap();

    assert!(
        second.is_none(),
        "host auth.json removal must return None regardless of staged mtime"
    );
    assert!(
        !staged_auth.exists(),
        "staged auth.json must be deleted on host revocation regardless of its mtime"
    );
    assert!(
        !dir.join("config.toml").exists(),
        "staged config.toml must be deleted alongside auth.json on host revocation"
    );
}
