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

#[cfg(unix)]
use std::os::unix::fs::symlink;

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

    let result = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let result = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let result = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    assert!(dir.join("auth.json").is_file());
    assert!(dir.join("config.toml").is_file());

    // Host revokes auth (e.g. `codex logout`): auth.json is gone.
    fs::remove_file(home_dir.path().join(".codex/auth.json")).unwrap();

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    assert!(dir.join("config.toml").is_file());

    // Host drops config.toml (falls back to codex defaults) and rotates auth.json.
    fs::remove_file(home_dir.path().join(".codex/config.toml")).unwrap();
    fs::write(
        home_dir.path().join(".codex/auth.json"),
        r#"{"token":"ROTATED_AUTH"}"#,
    )
    .unwrap();

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");

    // Simulate the in-container codex rotating its refresh token: the staged
    // (rw-mounted) auth.json is rewritten and now carries a newer mtime than
    // the host's original, untouched copy.
    fs::write(&staged_auth, r#"{"token":"CONTAINER_REFRESHED"}"#).unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    set_mtime(&staged_auth, t0);

    // User re-authenticates on the host (e.g. `codex login`): the host copy
    // is rewritten with a strictly newer mtime than the staged copy.
    fs::write(&host_auth, r#"{"token":"HOST_RELOGIN"}"#).unwrap();
    set_mtime(&host_auth, t0 + Duration::from_secs(3_600));

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
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

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    let staged_auth = dir.join("auth.json");
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    fs::remove_file(&host_auth).unwrap();

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

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

// --- prepare_codex_mount: shared user-level stage survives disposable
// container cleanup (codex review round 4, P1) ---

#[test]
fn prepare_codex_mount_survives_disposable_runtime_dir_cleanup() {
    // round 4 P1: disposable runs (`--new` / worktree) delete
    // `<config_dir>/runtime/<container_name>/` wholesale on exit
    // (`std::fs::remove_dir_all(&ctx.runtime_dir)` in interactive.rs /
    // prompt.rs). Before this fix, the codex stage lived *inside* that
    // per-container runtime dir, so a container-refreshed auth.json (the
    // only valid copy once the refresh token has rotated) was destroyed
    // along with it. The fix moves the stage to `<config_dir>/codex/`,
    // structurally outside anything a per-container cleanup ever touches.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let staged = prepare_codex_mount(home_dir.path(), config_dir.path())
        .unwrap()
        .expect("auth.json is present on the host, so a stage dir must be returned");

    assert_eq!(
        staged,
        config_dir.path().join("codex"),
        "codex stage must live directly under <config_dir>/codex, not under \
         <config_dir>/runtime/<container_name>/, so per-container cleanup never touches it"
    );

    // Simulate the in-container codex rotating the refresh token: the staged
    // (rw-mounted) auth.json is rewritten to a value that exists nowhere else.
    let staged_auth = staged.join("auth.json");
    fs::write(
        &staged_auth,
        r#"{"token":"CONTAINER_REFRESHED_BEFORE_CLEANUP"}"#,
    )
    .unwrap();

    // Simulate a disposable run's exact cleanup call: create a per-container
    // runtime dir (as `prepare_context` would for temp claude.json / sanitized
    // settings), then wholesale-remove it exactly like
    // `interactive.rs:169` / `prompt.rs:482` do.
    let runtime_dir = config_dir.path().join("runtime").join("vibepod-disposable");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(runtime_dir.join(".claude.json"), "{}").unwrap();
    fs::remove_dir_all(&runtime_dir).ok();

    assert!(
        !runtime_dir.exists(),
        "sanity check: the simulated per-container runtime dir must actually be gone"
    );
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED_BEFORE_CLEANUP"}"#,
        "the shared codex stage (and its container-refreshed auth.json) must survive \
         disposable per-container runtime dir cleanup"
    );
}

// --- prepare_codex_mount: symlink tampering defenses (codex review round 5, P1-a) ---

#[cfg(unix)]
#[test]
fn prepare_codex_mount_replaces_staged_symlink_without_following_it() {
    // A container with the rw-mounted stage could swap the staged auth.json
    // for a symlink pointing at an arbitrary host path, hoping the next
    // `vibepod run` copy follows it and overwrites that host file. This must
    // never happen: the symlink must be detected (without following) and
    // removed, then replaced with a fresh copy from the host.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");

    // A "victim" file outside the stage, standing in for an arbitrary host
    // path a container might target (e.g. ~/.ssh/authorized_keys).
    let victim = home_dir.path().join("victim.txt");
    fs::write(&victim, "VICTIM_UNCHANGED").unwrap();

    fs::remove_file(&staged_auth).unwrap();
    symlink(&victim, &staged_auth).unwrap();
    assert!(
        fs::symlink_metadata(&staged_auth)
            .unwrap()
            .file_type()
            .is_symlink(),
        "sanity check: staged auth.json must actually be a symlink before the next run"
    );

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

    assert_eq!(second, Some(dir.clone()));
    assert!(
        !fs::symlink_metadata(&staged_auth)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the symlink must be removed rather than followed, leaving a regular file in its place"
    );
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"HOST_AUTH"}"#,
        "staged auth.json must be re-copied from the host after the symlink is removed"
    );
    assert_eq!(
        fs::read_to_string(&victim).unwrap(),
        "VICTIM_UNCHANGED",
        "the symlink target (standing in for an arbitrary host file) must never be written to"
    );
}

#[cfg(unix)]
#[test]
fn prepare_codex_mount_removes_staged_symlink_pointing_nowhere() {
    // A symlink to a nonexistent path must also be detected via
    // symlink_metadata (which never follows) and removed cleanly, rather
    // than e.g. mistakenly treated as "file absent" in a way that bypasses
    // the tampering-removal path.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage config.toml");
    let staged_config = dir.join("config.toml");

    fs::remove_file(&staged_config).unwrap();
    symlink(home_dir.path().join("does-not-exist"), &staged_config).unwrap();

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

    assert_eq!(second, Some(dir));
    assert!(
        !fs::symlink_metadata(&staged_config)
            .unwrap()
            .file_type()
            .is_symlink(),
        "a dangling symlink must be removed and replaced with a regular file"
    );
    assert_eq!(
        fs::read_to_string(&staged_config).unwrap(),
        "model = \"gpt\"\n"
    );
}

// --- prepare_codex_mount: full reconciliation removes non-allowlisted entries
// (codex review round 5, P1-b) ---

#[test]
fn prepare_codex_mount_removes_non_allowlisted_files_and_directories_from_stage() {
    // The stage directory is rw bind-mounted into the container, so a
    // container process can create arbitrary files/directories there
    // (history.jsonl, a cache/ dir, ...). Before round 5 the reconcile step
    // only looked at the allowlist names, so anything else silently
    // persisted forever. It must now be swept on every `prepare_codex_mount`
    // call regardless of name.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");

    // Simulate a container writing allowlist-external junk directly into the
    // shared, rw-mounted stage.
    fs::write(dir.join("history.jsonl"), "SECRET_HISTORY_FROM_CONTAINER").unwrap();
    fs::create_dir_all(dir.join("cache")).unwrap();
    fs::write(dir.join("cache/entry"), "SECRET_CACHE_FROM_CONTAINER").unwrap();

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

    assert_eq!(second, Some(dir.clone()));
    assert!(
        !dir.join("history.jsonl").exists(),
        "non-allowlisted file must be removed by full reconciliation"
    );
    assert!(
        !dir.join("cache").exists(),
        "non-allowlisted directory must be removed (remove_dir_all) by full reconciliation"
    );

    // Round 1-4 regression: allowlisted files must survive the sweep.
    assert!(dir.join("auth.json").is_file());
    assert!(dir.join("config.toml").is_file());
    assert_eq!(
        fs::read_to_string(dir.join("auth.json")).unwrap(),
        r#"{"token":"HOST_AUTH"}"#
    );
    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).unwrap(),
        "model = \"gpt\"\n"
    );
}

// --- prepare_codex_mount: full reconciliation also sweeps stale fixed-name
// tmp symlinks before any copy runs (codex review round 5 regression check;
// round 6 P1's actual defense against copy_codex_asset_atomically itself
// following such a symlink is covered directly by
// copy_codex_asset_atomically_ignores_hostile_fixed_name_tmp_symlink in
// src/cli/run/mod.rs, since this integration-level test's reconcile step
// removes the planted symlink before the copy machinery ever runs and so
// cannot, by itself, tell a fixed-name-tmp implementation apart from the
// current unique-name one) ---

#[cfg(unix)]
#[test]
fn prepare_codex_mount_reconcile_sweeps_stale_fixed_name_tmp_symlink_before_copy() {
    // A container could leave a fixed-name `auth.json.tmp` symlink behind in
    // the rw-mounted stage (e.g. as a leftover from tampering, or targeting
    // an arbitrary host path such as `~/.ssh/authorized_keys`). This test
    // confirms that by the time the *next* `vibepod run` reaches the copy
    // step, round 5's full reconciliation (`reconcile_codex_stage_dir`,
    // which runs before any copy) has already swept it away as a
    // non-allowlisted entry, so it can never be reachable by the copy
    // machinery in the first place.
    //
    // This is a defense-in-depth regression check on the reconcile step, not
    // a test of copy_codex_asset_atomically's own symlink handling: because
    // reconcile removes the symlink before copy ever sees it, this test
    // would still pass even if copy_codex_asset_atomically itself still used
    // a fixed-name tmp file. That specific defense is verified directly in
    // src/cli/run/mod.rs by
    // copy_codex_asset_atomically_ignores_hostile_fixed_name_tmp_symlink,
    // which calls copy_codex_asset_atomically without going through
    // reconcile.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    set_mtime(&staged_auth, t0);

    // A "victim" file outside the stage, standing in for an arbitrary host
    // path a container might target via the fixed-name tmp file (e.g.
    // ~/.ssh/authorized_keys).
    let victim = home_dir.path().join("victim.txt");
    fs::write(&victim, "VICTIM_UNCHANGED").unwrap();

    // Plant the hostile fixed-name tmp file that the pre-round-6
    // implementation would have written through, pointing it at the victim.
    let hostile_tmp = dir.join("auth.json.tmp");
    symlink(&victim, &hostile_tmp).unwrap();
    assert!(
        fs::symlink_metadata(&hostile_tmp)
            .unwrap()
            .file_type()
            .is_symlink(),
        "sanity check: the hostile auth.json.tmp must actually be a symlink before the next run"
    );

    // User re-authenticates on the host: the host copy is rewritten with a
    // strictly newer mtime, forcing the next `prepare_codex_mount` to
    // actually perform the copy (rather than keep the container-refreshed
    // staged copy per round 3), so the copy path under test really executes.
    fs::write(&host_auth, r#"{"token":"HOST_RELOGIN"}"#).unwrap();
    set_mtime(&host_auth, t0 + Duration::from_secs(3_600));

    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();

    assert_eq!(second, Some(dir.clone()));
    assert_eq!(
        fs::read_to_string(&victim).unwrap(),
        "VICTIM_UNCHANGED",
        "the hostile fixed-name tmp file's symlink target must never be written to, \
         regardless of whether stage reconciliation or the copy machinery itself is what \
         keeps it safe"
    );
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"HOST_RELOGIN"}"#,
        "auth.json must still be correctly staged from the host copy despite the hostile \
         fixed-name tmp file sharing its directory"
    );
}

// --- prepare_codex_mount: concurrent stage preparation is serialized by a
// flock so that neither run's full reconciliation can delete the other's
// in-flight NamedTempFile (codex review round 7, P1) ---

#[test]
fn prepare_codex_mount_completes_reconcile_and_copy_under_stage_lock() {
    // round 7 regression guard: prepare_codex_mount now acquires
    // <config_dir>/codex.lock around its entire body (reconcile + copy loop).
    // This confirms the lock is acquired and released cleanly across two
    // ordinary sequential calls -- if acquisition or release were broken
    // (e.g. the guard leaked past the function's return, or the lock file
    // path collided with an allowlisted entry), the second call would hang
    // or fail rather than complete normally.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    let dir = first.expect("first call must stage auth.json + config.toml under the lock");
    assert!(dir.join("auth.json").is_file());
    assert!(dir.join("config.toml").is_file());

    // A second call must not hang (lock was released after the first call)
    // and must reconcile + copy again without error.
    let second = prepare_codex_mount(home_dir.path(), config_dir.path()).unwrap();
    assert_eq!(second, Some(dir.clone()));
    assert_eq!(
        fs::read_to_string(dir.join("auth.json")).unwrap(),
        r#"{"token":"HOST_AUTH"}"#
    );

    // The lock file itself must live outside the stage directory, so the
    // stage's own full reconciliation (round 5) never treats it as a stale,
    // non-allowlisted entry to sweep.
    assert!(
        config_dir.path().join("codex.lock").is_file(),
        "codex.lock must exist directly under config_dir"
    );
    assert!(
        !dir.join("codex.lock").exists(),
        "codex.lock must never live inside the codex stage dir itself, or the stage's own \
         full reconciliation would delete it"
    );
}

#[test]
fn prepare_codex_mount_succeeds_under_concurrent_calls_to_same_stage() {
    // round 7 P1: two `vibepod run` invocations preparing the same shared
    // stage at the same time used to race -- one thread's full
    // reconciliation could delete the other's in-flight NamedTempFile,
    // making chmod/persist fail intermittently with NotFound.
    //
    // A single iteration of this race is weak as a regression guard: manual
    // measurement (reviewer, round 7 follow-up) with the stage lock
    // temporarily disabled showed the race only reproduces in ~2-3% of single
    // runs (50-80 attempts needed one hit), so a lone iteration would pass CI
    // almost every time even if the lock's scope were accidentally narrowed
    // back down in a future change. Looping the same two-thread call
    // `ITERATIONS` times inside one test process turns that ~2-3% per-attempt
    // chance into a near-certain detection within a single `cargo test` run,
    // while still finishing in well under a second.
    //
    // Both threads in every iteration copy from the *same* host `~/.codex/`
    // (identical content), which keeps the expected final state deterministic
    // regardless of which thread's copy physically lands last -- each
    // iteration only needs the two calls to never error, not to race on
    // distinguishable content, so it can't be flaky on outcome.
    const ITERATIONS: usize = 300;

    for iteration in 0..ITERATIONS {
        // Fresh tempdirs per iteration: reusing the previous iteration's
        // stage/config dir would let leftover state (e.g. an already-staged
        // auth.json) mask a broken lock in a later iteration.
        let home_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        make_host_codex(home_dir.path());

        let home_a = home_dir.path().to_path_buf();
        let config_a = config_dir.path().to_path_buf();
        let home_b = home_a.clone();
        let config_b = config_a.clone();

        let handle_a = std::thread::spawn(move || prepare_codex_mount(&home_a, &config_a));
        let handle_b = std::thread::spawn(move || prepare_codex_mount(&home_b, &config_b));

        let result_a = handle_a.join().expect("thread A must not panic");
        let result_b = handle_b.join().expect("thread B must not panic");

        assert!(
            result_a.is_ok(),
            "iteration {iteration}: concurrent prepare_codex_mount A must not fail on the \
             other thread's in-flight temp file being reconciled away: {:?}",
            result_a.err()
        );
        assert!(
            result_b.is_ok(),
            "iteration {iteration}: concurrent prepare_codex_mount B must not fail on the \
             other thread's in-flight temp file being reconciled away: {:?}",
            result_b.err()
        );

        let dir_a = result_a
            .unwrap()
            .expect("auth.json is present on the host, so Some(dir) is expected");
        let dir_b = result_b
            .unwrap()
            .expect("auth.json is present on the host, so Some(dir) is expected");
        assert_eq!(dir_a, dir_b, "iteration {iteration}");
        assert_eq!(
            dir_a,
            config_dir.path().join("codex"),
            "iteration {iteration}"
        );

        assert_eq!(
            fs::read_to_string(dir_a.join("auth.json")).unwrap(),
            r#"{"token":"HOST_AUTH"}"#,
            "iteration {iteration}: both threads copy the same host auth.json, so the final \
             staged content must match it regardless of which thread's copy physically lands \
             last"
        );
        assert_eq!(
            fs::read_to_string(dir_a.join("config.toml")).unwrap(),
            "model = \"gpt\"\n",
            "iteration {iteration}: both threads copy the same host config.toml, so the final \
             staged content must match it regardless of which thread's copy physically lands \
             last"
        );
    }
}
