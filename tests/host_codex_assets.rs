//! Host `~/.codex/` assets copied into the container for codex review support.
//!
//! The invariant under test: only an allowlisted subset of `~/.codex/`
//! (`auth.json` / `config.toml`) is ever copied into the per-container
//! runtime directory that gets mounted as `/home/vibepod/.codex`. Session
//! history (`history.jsonl`), goal databases (`goals_*.sqlite`), and any
//! `cache/` contents must never travel in — they are sensitive and
//! unnecessary for running `codex`.
//!
//! **round 10 architecture** ("見せる領域" と "永続化する領域" の分離):
//! `prepare_codex_mount(home, config_dir, runtime_dir)` now syncs in two hops:
//! host `~/.codex/` -> **auth store** (`<config_dir>/codex-auth/`, host-only,
//! never mounted into a container) -> **per-container stage**
//! (`<runtime_dir>/codex/`, the directory actually bind-mounted as
//! `/home/vibepod/.codex`). The stage is disposable — it is deleted wholesale
//! with `runtime_dir` on disposable-run cleanup — while the auth store is the
//! only place a refreshed token survives across runs. `sync_codex_stage_to_store`
//! is the run-after counterpart that writes a container-refreshed stage
//! `auth.json` back into the store before that cleanup happens.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use vibepod::cli::run::{
    host_codex_stage_entries, prepare_codex_mount, should_keep_staged_auth,
    sync_codex_stage_to_store, ContainerLiveness,
};
use vibepod::runtime::ContainerConfig;

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
    let runtime_dir = tempfile::tempdir().unwrap();

    let result =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    let codex_dir = home_dir.path().join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(codex_dir.join("config.toml"), "model = \"gpt\"\n").unwrap();

    let result =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

    assert!(
        result.is_none(),
        "should return None when auth.json is absent even if config.toml exists"
    );
}

#[test]
fn prepare_codex_mount_copies_both_files_and_returns_dir() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let result =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    assert!(dir.join("auth.json").is_file());
    assert!(dir.join("config.toml").is_file());

    // round 10: the first hop (host -> store) must have populated the
    // host-only auth store too, not just the per-container stage.
    let store_dir = config_dir.path().join("codex-auth");
    assert!(
        store_dir.join("auth.json").is_file(),
        "the first hop (host -> store) must populate the auth store as well as the stage"
    );
    assert!(store_dir.join("config.toml").is_file());

    // Host revokes auth (e.g. `codex logout`): auth.json is gone.
    fs::remove_file(home_dir.path().join(".codex/auth.json")).unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    // round 10: host revocation must wipe the auth store as well as the
    // stage -- otherwise a stale store copy would silently repopulate the
    // stage of the *next* container to run, even though the host has
    // revoked auth.
    assert!(
        !store_dir.join("auth.json").exists(),
        "the auth store's stale auth.json must be wiped alongside the stage on host revocation"
    );
    assert!(
        !store_dir.join("config.toml").exists(),
        "the auth store's stale config.toml must be wiped alongside the stage on host revocation"
    );
    assert!(
        store_dir.is_dir(),
        "the auth store directory itself must survive the wipe (only its contents are cleared)"
    );
}

#[test]
fn prepare_codex_mount_reconciles_config_toml_removal_and_refreshes_auth() {
    // First run stages both files into the runtime dir.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    assert!(dir.join("config.toml").is_file());

    // Host drops config.toml (falls back to codex defaults) and rotates auth.json.
    fs::remove_file(home_dir.path().join(".codex/config.toml")).unwrap();
    fs::write(
        home_dir.path().join(".codex/auth.json"),
        r#"{"token":"ROTATED_AUTH"}"#,
    )
    .unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
fn should_keep_staged_auth_true_when_mtimes_are_equal() {
    // codex review round 8 P2-1: equal mtimes now fall through to "staged
    // wins" (keep), reversing the pre-round-8 "host wins" behavior. Rationale:
    // staging a copy and an in-container refresh token rotation can collapse
    // to the same mtime within a coarse filesystem's timestamp resolution
    // (e.g. whole-second granularity). The equal-mtime case where the host is
    // *actually* newer (a re-login landing at the exact same instant) is
    // astronomically rare and, if it does happen, is trivially recoverable by
    // logging in again. Losing a rotated refresh token by overwriting it
    // instead is a much heavier failure that requires user action to recover
    // from, so the safe default is to keep the staged copy on a tie.
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(should_keep_staged_auth(t, t));
}

// --- prepare_codex_mount: keep-newer-auth (codex review round 3, P1) ---

#[test]
fn prepare_codex_mount_keeps_staged_auth_when_newer_than_host() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");

    // Simulate the in-container codex rotating its refresh token: the staged
    // (rw-mounted) auth.json is rewritten and now carries a newer mtime than
    // the host's original, untouched copy.
    fs::write(&staged_auth, r#"{"token":"CONTAINER_REFRESHED"}"#).unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    set_mtime(&staged_auth, t0);

    // User re-authenticates on the host (e.g. `codex login`): the host copy
    // is rewritten with a strictly newer mtime than the staged copy.
    fs::write(&host_auth, r#"{"token":"HOST_RELOGIN"}"#).unwrap();
    set_mtime(&host_auth, t0 + Duration::from_secs(3_600));

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_config = home_dir.path().join(".codex/config.toml");

    let t0 = SystemTime::now();
    set_mtime(&host_config, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
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

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    let staged_auth = dir.join("auth.json");
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    fs::remove_file(&host_auth).unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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

// --- prepare_codex_mount / sync_codex_stage_to_store: disposable runtime_dir
// cleanup destroys the per-container stage, but a run-after sync leaves the
// refreshed auth.json recoverable in the host-only auth store (codex review
// round 10 — this test's *meaning* is inverted from the pre-round-10 version:
// back then the stage itself had to survive cleanup; now it is expected and
// fine for the stage to be destroyed, because persistence has moved to a
// structurally separate auth store) ---

#[test]
fn prepare_codex_mount_stage_is_disposed_but_auth_store_survives_runtime_dir_cleanup() {
    // round 10: the stage (`<runtime_dir>/codex/`) is a per-container,
    // disposable staging area — disposable runs (`--new` / worktree) delete
    // `<config_dir>/runtime/<container_name>/` wholesale on exit
    // (`std::fs::remove_dir_all(&ctx.runtime_dir)` in interactive.rs /
    // prompt.rs), taking the stage with it. That is fine *only* because
    // `sync_codex_stage_to_store` is called first (both call sites do this
    // before the removal) to write any container-refreshed auth.json back
    // into the auth store (`<config_dir>/codex-auth/`), which lives outside
    // `runtime_dir` and is never bind-mounted into any container.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");
    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let staged = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("auth.json is present on the host, so a stage dir must be returned");

    assert_eq!(
        staged,
        runtime_dir.path().join("codex"),
        "the stage must live under <runtime_dir>/codex (per-container), not under \
         <config_dir>/codex — <config_dir> now holds only the host-only auth store"
    );

    // Simulate the in-container codex rotating the refresh token: the staged
    // (rw-mounted) auth.json is rewritten to a value that, before the
    // run-after sync below, exists nowhere else.
    let staged_auth = staged.join("auth.json");
    fs::write(
        &staged_auth,
        r#"{"token":"CONTAINER_REFRESHED_BEFORE_CLEANUP"}"#,
    )
    .unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    // Simulate the run-after sync that both `interactive.rs` and `prompt.rs`
    // call before their cleanup branch removes `runtime_dir`.
    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .expect("sync_codex_stage_to_store must succeed before disposable cleanup");

    let store_auth = config_dir.path().join("codex-auth").join("auth.json");
    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED_BEFORE_CLEANUP"}"#,
        "the run-after sync must have written the container-refreshed auth.json into the \
         host-only auth store before cleanup destroys the stage"
    );

    // Simulate a disposable run's exact cleanup call:
    // `std::fs::remove_dir_all(&ctx.runtime_dir)` in interactive.rs / prompt.rs.
    fs::remove_dir_all(runtime_dir.path()).unwrap();

    assert!(
        !staged_auth.exists(),
        "sanity check: the per-container stage must actually be gone after cleanup — losing \
         it is expected and fine, since the refreshed value now lives in the auth store"
    );
    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED_BEFORE_CLEANUP"}"#,
        "the host-only auth store must survive disposable per-container runtime_dir cleanup, \
         carrying the container-refreshed auth.json forward to the next `vibepod run`"
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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
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

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage config.toml");
    let staged_config = dir.join("config.toml");

    fs::remove_file(&staged_config).unwrap();
    symlink(home_dir.path().join("does-not-exist"), &staged_config).unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");

    // Simulate a container writing allowlist-external junk directly into the
    // shared, rw-mounted stage.
    fs::write(dir.join("history.jsonl"), "SECRET_HISTORY_FROM_CONTAINER").unwrap();
    fs::create_dir_all(dir.join("cache")).unwrap();
    fs::write(dir.join("cache/entry"), "SECRET_CACHE_FROM_CONTAINER").unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
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

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

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
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first call must stage auth.json + config.toml under the lock");
    assert!(dir.join("auth.json").is_file());
    assert!(dir.join("config.toml").is_file());

    // A second call must not hang (lock was released after the first call)
    // and must reconcile + copy again without error.
    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
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
fn prepare_codex_mount_succeeds_under_concurrent_calls_from_different_containers() {
    // round 7 P1 found this race when the stage was a single directory
    // shared by every container: two concurrent `vibepod run` invocations
    // preparing it at the same time could have one thread's full
    // reconciliation delete the other's in-flight NamedTempFile, making
    // chmod/persist fail intermittently with NotFound.
    //
    // round 10 moved the stage to be per-container (`<runtime_dir>/codex/`,
    // isolated by container name), which removes that particular race: two
    // containers no longer share a stage directory at all. But it introduced
    // a *new* shared resource in its place -- the auth store
    // (`<config_dir>/codex-auth/`) -- which every `prepare_codex_mount` call
    // syncs through (host -> store -> stage) regardless of which container's
    // stage it is ultimately populating. Two different containers' `vibepod
    // run` starting at the same instant now race on that shared store
    // instead, so this test simulates exactly that: two threads, each with
    // its own `runtime_dir` (a distinct container), sharing one `config_dir`
    // (and therefore one auth store). The flock that round 7 added -- moved
    // in round 10 to guard the store instead of the stage -- must still
    // serialize the two store-touching hops so neither thread's reconciliation
    // corrupts the other's in-flight write.
    //
    // A single iteration of this race is weak as a regression guard: manual
    // measurement (reviewer, round 7 follow-up) with the lock temporarily
    // disabled showed the race only reproduces in ~2-3% of single runs
    // (50-80 attempts needed one hit), so a lone iteration would pass CI
    // almost every time even if the lock's scope were accidentally narrowed
    // back down in a future change. Looping the same two-thread call
    // `ITERATIONS` times inside one test process turns that ~2-3% per-attempt
    // chance into a near-certain detection within a single `cargo test` run.
    //
    // Both threads in every iteration copy from the *same* host `~/.codex/`
    // (identical content), which keeps the expected final state deterministic
    // regardless of which thread's copy physically lands last in the shared
    // store -- each iteration only needs the two calls to never error, not to
    // race on distinguishable content, so it can't be flaky on outcome.
    const ITERATIONS: usize = 300;

    for iteration in 0..ITERATIONS {
        // Fresh tempdirs per iteration: reusing the previous iteration's
        // config/runtime dirs would let leftover state (e.g. an
        // already-populated auth store) mask a broken lock in a later
        // iteration.
        let home_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let runtime_dir_a = tempfile::tempdir().unwrap();
        let runtime_dir_b = tempfile::tempdir().unwrap();
        make_host_codex(home_dir.path());

        let home_a = home_dir.path().to_path_buf();
        let config_a = config_dir.path().to_path_buf();
        let runtime_a = runtime_dir_a.path().to_path_buf();
        let home_b = home_a.clone();
        let config_b = config_a.clone();
        let runtime_b = runtime_dir_b.path().to_path_buf();

        let handle_a =
            std::thread::spawn(move || prepare_codex_mount(&home_a, &config_a, &runtime_a));
        let handle_b =
            std::thread::spawn(move || prepare_codex_mount(&home_b, &config_b, &runtime_b));

        let result_a = handle_a.join().expect("thread A must not panic");
        let result_b = handle_b.join().expect("thread B must not panic");

        assert!(
            result_a.is_ok(),
            "iteration {iteration}: concurrent prepare_codex_mount A (container A) must not \
             fail on the other container's in-flight auth-store write being reconciled away: \
             {:?}",
            result_a.err()
        );
        assert!(
            result_b.is_ok(),
            "iteration {iteration}: concurrent prepare_codex_mount B (container B) must not \
             fail on the other container's in-flight auth-store write being reconciled away: \
             {:?}",
            result_b.err()
        );

        let dir_a = result_a
            .unwrap()
            .expect("auth.json is present on the host, so Some(dir) is expected");
        let dir_b = result_b
            .unwrap()
            .expect("auth.json is present on the host, so Some(dir) is expected");

        // Different containers must get independent, non-shared stage dirs
        // -- this is precisely the round 10 change that eliminated the
        // pre-round-10 stage-level race.
        assert_eq!(
            dir_a,
            runtime_dir_a.path().join("codex"),
            "iteration {iteration}"
        );
        assert_eq!(
            dir_b,
            runtime_dir_b.path().join("codex"),
            "iteration {iteration}"
        );
        assert_ne!(
            dir_a, dir_b,
            "iteration {iteration}: two different containers must never be handed the same \
             stage directory"
        );

        for (label, dir) in [("A", &dir_a), ("B", &dir_b)] {
            assert_eq!(
                fs::read_to_string(dir.join("auth.json")).unwrap(),
                r#"{"token":"HOST_AUTH"}"#,
                "iteration {iteration}: container {label}'s stage must reflect the shared \
                 host auth.json regardless of which thread's auth-store write landed last"
            );
            assert_eq!(
                fs::read_to_string(dir.join("config.toml")).unwrap(),
                "model = \"gpt\"\n",
                "iteration {iteration}: container {label}'s stage must reflect the shared \
                 host config.toml regardless of which thread's auth-store write landed last"
            );
        }

        // The shared auth store behind both stages must itself have
        // converged on the host's content without corruption from the
        // concurrent access.
        let store_auth = config_dir.path().join("codex-auth").join("auth.json");
        assert_eq!(
            fs::read_to_string(&store_auth).unwrap(),
            r#"{"token":"HOST_AUTH"}"#,
            "iteration {iteration}: the auth store shared by both containers must end up with \
             the host's content, not a corrupted interleaving of the two concurrent writes"
        );
    }
}

// --- prepare_codex_mount: non-regular-file staged auth.json is removed and
// re-created from host (codex review round 8, P1) ---

#[test]
fn prepare_codex_mount_replaces_staged_auth_directory_with_fresh_host_copy() {
    // round 8 P1: `reconcile_codex_stage_dir`'s sweep only inspects entry
    // *names* against the allowlist -- it `continue`s past any entry whose
    // name matches an allowlisted name (see the `keep.contains(&name_str...)`
    // early-continue in that function) without ever looking at its
    // `file_type`. So if the container replaces the staged `auth.json` with a
    // directory of the same name, reconcile's sweep never sees it as "stale";
    // only the dedicated non-regular-file defense
    // (`remove_dst_if_not_regular_file`, called per-entry right before the
    // mtime/copy logic) can catch it. If this understanding were wrong --
    // e.g. if reconcile alone already handled this -- this test would give a
    // false positive by accidentally passing for the wrong reason, so this
    // comment records the structural reason it must be the dedicated defense
    // doing the work here.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    assert!(
        staged_auth.is_file(),
        "sanity check: first run stages a regular file"
    );

    // A container with the rw-mounted stage swaps the staged auth.json for a
    // directory of the same name. A directory's mtime can easily be "newer"
    // than the host's untouched auth.json, which (absent this defense) would
    // let it survive should_keep_staged_auth's keep-newer check forever.
    fs::remove_file(&staged_auth).unwrap();
    fs::create_dir_all(&staged_auth).unwrap();
    fs::write(staged_auth.join("junk"), "not an auth.json").unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

    assert_eq!(second, Some(dir.clone()));
    assert!(
        staged_auth.is_file(),
        "the directory masquerading as auth.json must be removed and replaced with a regular \
         file"
    );
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"HOST_AUTH"}"#,
        "staged auth.json must be re-copied from the host after the directory is removed"
    );
}

// --- prepare_codex_mount: equal mtimes keep the staged copy (codex review
// round 8, P2-1) ---

#[test]
fn prepare_codex_mount_keeps_staged_auth_when_mtimes_are_equal() {
    // round 8 P2-1 integration guard: staging a copy and an in-container
    // refresh landing within the same fs timestamp granularity can produce
    // equal mtimes. Before round 8 this fell through to "host wins" and
    // silently discarded the container's rotated refresh token; now it must
    // keep the staged copy instead.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    // round 10: pin the *store's* mtime (the intermediate hop between host
    // and stage) to exactly t0 too. `copy_codex_asset_atomically`'s persist
    // during the first call above stamps the store's auth.json with the
    // real wall-clock time at that moment, which is a hair after `t0` was
    // captured -- without this, hop 2 (store -> stage) would see the
    // store's mtime as strictly newer than the `t0` this test pins the
    // stage to below, taking the "host [store] wins" branch and silently
    // overwriting the container's refreshed stage copy with the store's
    // (host-derived) content. That would defeat the very equal-mtime
    // semantics this test exists to guard (round 8 P2-1).
    set_mtime(&store_auth, t0);

    // Simulate the in-container codex rotating its refresh token at exactly
    // the same fs-visible timestamp as the host's copy (coarse mtime
    // resolution collapsing two genuinely different instants into one
    // value).
    fs::write(
        &staged_auth,
        r#"{"token":"CONTAINER_REFRESHED_SAME_INSTANT"}"#,
    )
    .unwrap();
    set_mtime(&staged_auth, t0);

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

    assert_eq!(second, Some(dir));
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED_SAME_INSTANT"}"#,
        "equal mtimes must keep the staged copy rather than fall back to overwriting it with \
         the host copy"
    );
}

// --- prepare_codex_mount: copy is skipped when staged content already
// matches host (codex review round 8, P2-2) ---

#[test]
fn prepare_codex_mount_skips_copy_when_staged_auth_content_matches_host() {
    // round 8 P2-2: even when should_keep_staged_auth takes the "host wins"
    // branch (host mtime is strictly newer than staged), the copy itself must
    // still be skipped if the staged content is already byte-identical to the
    // host's -- no chmod, no rename, no mtime bump.
    //
    // This is verified indirectly via mtime: the staged auth.json's mtime is
    // pinned to a value strictly older than the host's (forcing
    // should_keep_staged_auth's "host wins" branch, so the copy machinery's
    // own content check is what's actually under test, not P2-1's mtime
    // short-circuit). If the copy ran, `copy_codex_asset_atomically`'s
    // NamedTempFile::persist would replace the destination inode and the OS
    // would stamp a fresh mtime at persist time -- not the exact value pinned
    // beforehand. So an unchanged mtime after the call is proof the copy path
    // was never taken.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");

    // The staged content is already identical to the host's: make_host_codex
    // writes the same auth.json content on the host, and the first
    // prepare_codex_mount call above copied it verbatim -- neither side has
    // been rewritten since. Pin the staged mtime strictly older than the
    // host's so should_keep_staged_auth takes the "host wins" branch and the
    // copy machinery's content-equality check is what's actually exercised.
    let staged_mtime = t0 - Duration::from_secs(60);
    set_mtime(&staged_auth, staged_mtime);
    let mtime_before = fs::metadata(&staged_auth).unwrap().modified().unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

    assert_eq!(second, Some(dir));
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"HOST_AUTH"}"#,
        "content must remain the (already-matching) host value"
    );
    let mtime_after = fs::metadata(&staged_auth).unwrap().modified().unwrap();
    assert_eq!(
        mtime_after, mtime_before,
        "identical content must skip the copy entirely, leaving the staged file's mtime \
         untouched rather than bumped by a fresh persist"
    );
}

// --- prepare_codex_mount: staged permissions are repaired even when the
// content is kept or the copy is skipped (codex review round 9, P1) ---

#[cfg(unix)]
#[test]
fn prepare_codex_mount_restores_0600_on_kept_staged_auth_with_loosened_permissions() {
    // round 9 P1: the keep-newer branch (`should_keep_staged_auth` returning
    // true, so the loop `continue`s past the staged auth.json without
    // touching it) intentionally preserves content and mtime -- but it must
    // not also preserve a *loosened* permission bit. If a container reached
    // through the shared rw mount and `chmod 644`'d the staged auth.json (a
    // token readable by any other host user is exactly the leak this
    // invariant guards against), the next `vibepod run` must still repair the
    // mode to 0600 even though it takes the "keep, don't overwrite" path for
    // the content itself.
    use std::os::unix::fs::PermissionsExt;

    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");

    // Simulate an in-container refresh (newer mtime, new content) that also
    // leaves the file world/group-readable -- e.g. the container process ran
    // under a different umask than the 0600 vibepod sets on its own copies.
    fs::write(
        &staged_auth,
        r#"{"token":"CONTAINER_REFRESHED_WITH_LOOSE_PERMS"}"#,
    )
    .unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(60));
    fs::set_permissions(&staged_auth, fs::Permissions::from_mode(0o644)).unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

    assert_eq!(second, Some(dir));
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED_WITH_LOOSE_PERMS"}"#,
        "the keep-newer branch must still preserve the container-refreshed content"
    );
    let mode = fs::metadata(&staged_auth).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the keep-newer branch must repair a loosened permission bit back to 0600, got {:o}",
        mode
    );
}

#[cfg(unix)]
#[test]
fn prepare_codex_mount_restores_0600_when_copy_is_skipped_for_matching_content() {
    // round 9 P1: `copy_codex_asset_atomically`'s content-equality short
    // circuit (`staged_asset_matches_host` returning true, round 8 P2-2)
    // skips copy/chmod/rename entirely when the staged file already matches
    // the host byte-for-byte. That short circuit predates any container
    // access to the stage, so it never re-applies 0600 on its own. If the
    // container had already loosened the staged file's mode before this run
    // (content still identical, so the skip still fires), the loosened mode
    // must not survive -- `prepare_codex_mount`'s end-of-function permission
    // sweep has to catch what this early return does not.
    use std::os::unix::fs::PermissionsExt;

    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");

    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");

    // Content is already identical to the host (copied verbatim by the first
    // call above). Pin the staged mtime strictly older than the host's so
    // `should_keep_staged_auth` takes the "host wins" branch, which forces
    // the content-equality check inside `copy_codex_asset_atomically` to be
    // what actually decides to skip the copy.
    let staged_mtime = t0 - Duration::from_secs(60);
    set_mtime(&staged_auth, staged_mtime);
    fs::set_permissions(&staged_auth, fs::Permissions::from_mode(0o644)).unwrap();
    let mtime_before = fs::metadata(&staged_auth).unwrap().modified().unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();

    assert_eq!(second, Some(dir));
    assert_eq!(
        fs::read_to_string(&staged_auth).unwrap(),
        r#"{"token":"HOST_AUTH"}"#,
        "content must remain the (already-matching) host value"
    );
    let mtime_after = fs::metadata(&staged_auth).unwrap().modified().unwrap();
    assert_eq!(
        mtime_after, mtime_before,
        "the permission repair must not go through the copy machinery -- mtime must stay \
         untouched, proving the fix was a plain chmod rather than a re-copy/persist"
    );
    let mode = fs::metadata(&staged_auth).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the content-equality skip path must still have its permission repaired to 0600, got {:o}",
        mode
    );
}

// --- prepare_codex_mount: the host -> store hop gets the same full
// reconciliation defense as the store -> stage hop (codex review round 10;
// round 5 P1-b originally covered only the single host -> stage hop) ---

#[test]
fn prepare_codex_mount_reconciles_non_allowlisted_entries_directly_in_the_auth_store() {
    // round 10 introduced a second hop (host -> store -> stage), both driven
    // by the same `sync_codex_entries_into` helper. This confirms the
    // full-reconciliation sweep (round 5 P1-b: delete anything in the
    // destination that isn't in this run's allowlisted entries) is really
    // applied to the auth store directly, one hop upstream of the stage --
    // not just observable indirectly through the stage that existing tests
    // already cover.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let first =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    let dir = first.expect("first run should stage auth.json + config.toml");
    let store_dir = config_dir.path().join("codex-auth");
    assert!(store_dir.join("auth.json").is_file());

    // Simulate junk landing directly in the store (e.g. leftover from a bug
    // elsewhere, or a manual edit) rather than via the stage.
    fs::write(store_dir.join("history.jsonl"), "SECRET_HISTORY_IN_STORE").unwrap();
    fs::create_dir_all(store_dir.join("cache")).unwrap();
    fs::write(store_dir.join("cache/entry"), "SECRET_CACHE_IN_STORE").unwrap();

    let second =
        prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path()).unwrap();
    assert_eq!(second, Some(dir));

    assert!(
        !store_dir.join("history.jsonl").exists(),
        "non-allowlisted file must be swept from the auth store, not just from the stage"
    );
    assert!(
        !store_dir.join("cache").exists(),
        "non-allowlisted directory must be swept from the auth store, not just from the stage"
    );
    assert!(
        store_dir.join("auth.json").is_file(),
        "allowlisted auth.json must survive the store's own reconciliation sweep"
    );
    assert!(
        store_dir.join("config.toml").is_file(),
        "allowlisted config.toml must survive the store's own reconciliation sweep"
    );
}

// --- container mount args: the auth store must never be reachable through
// any docker mount argument (codex review round 10, "分離の検証") ---

#[test]
fn container_mount_args_expose_stage_but_never_the_auth_store_path() {
    // round 10's core invariant is that the auth store
    // (`<config_dir>/codex-auth/`) is host-only and never bind-mounted into
    // any container -- only the per-container stage (`<runtime_dir>/codex/`)
    // is. `build_container_config` (src/cli/run/mod.rs) copies
    // `ctx.codex_dir` (the stage) into `ContainerConfig.codex_dir`, and
    // `ContainerConfig::to_create_args` (src/runtime/docker.rs) is the
    // function that actually turns that into the `docker run -v ...`
    // argument. Exercising it directly here (rather than re-deriving the
    // invariant by inspecting fields) means a future change that
    // accidentally wires the store into a mount argument would fail this
    // test.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let stage_dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("auth.json is present on the host, so a stage dir must be returned");
    let store_dir = config_dir.path().join("codex-auth");
    assert!(
        store_dir.join("auth.json").is_file(),
        "sanity check: the auth store must actually have been populated by this run"
    );

    let config = ContainerConfig {
        image: "vibepod:test".to_string(),
        container_name: "vibepod-test-container".to_string(),
        workspace_path: "/tmp/workspace".to_string(),
        claude_json: None,
        codex_dir: Some(stage_dir.to_string_lossy().to_string()),
        gitconfig: None,
        env_vars: Vec::new(),
        network_disabled: false,
        extra_mounts: Vec::new(),
        labels: HashMap::new(),
    };

    let args = config.to_create_args();
    let joined = args.join(" ");

    assert!(
        joined.contains(&stage_dir.to_string_lossy().to_string()),
        "sanity check: the per-container stage path must appear in the generated mount args: {:?}",
        args
    );
    assert!(
        !joined.contains(&store_dir.to_string_lossy().to_string()),
        "the auth store path must never appear in any container mount argument: {:?}",
        args
    );
    assert!(
        !joined.contains("codex-auth"),
        "the literal auth store directory name must never leak into container mount arguments: {:?}",
        args
    );
}

// --- sync_codex_stage_to_store: run-after sync of a container-refreshed
// stage auth.json into the host-only auth store (codex review round 10) ---

#[test]
fn sync_codex_stage_to_store_promotes_newer_stage_auth_to_store() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());
    let host_auth = home_dir.path().join(".codex/auth.json");
    let t0 = SystemTime::now();
    set_mtime(&host_auth, t0);

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    // Simulate the in-container codex rotating the refresh token: the
    // rw-mounted stage auth.json is rewritten with a strictly newer mtime
    // than whatever the store currently holds.
    fs::write(&staged_auth, r#"{"token":"CONTAINER_REFRESHED"}"#).unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .expect("sync_codex_stage_to_store must succeed when promoting a newer stage auth.json");

    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED"}"#,
        "a newer stage auth.json must be promoted into the auth store"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&store_auth).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "the promoted auth.json in the store must be 0600, got {:o}",
            mode
        );
    }
}

#[test]
fn sync_codex_stage_to_store_skips_when_stage_auth_is_older_than_store() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    let t0 = SystemTime::now();
    // The stage is pinned strictly older than the store, simulating a store
    // that already holds a more recently synced value (e.g. written by a
    // different container's run-after sync) than this container's stage.
    set_mtime(&staged_auth, t0);
    fs::write(&store_auth, r#"{"token":"STORE_ALREADY_NEWER"}"#).unwrap();
    set_mtime(&store_auth, t0 + Duration::from_secs(3_600));

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .expect("sync_codex_stage_to_store must succeed even when it skips the copy");

    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"STORE_ALREADY_NEWER"}"#,
        "an older-than-store stage auth.json must never overwrite a newer store copy"
    );
}

#[test]
fn sync_codex_stage_to_store_skips_when_stage_auth_mtime_equals_store() {
    // `sync_codex_stage_to_store` reuses `should_keep_staged_auth`, called as
    // `should_keep_staged_auth(stage_mtime, store_mtime)` — a tie
    // (`store_mtime >= stage_mtime`) takes the "keep" branch, i.e. the store
    // is left alone. This matches the spec's "古い(または同値)→取り込まれない".
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    let t0 = SystemTime::now();
    set_mtime(&staged_auth, t0);
    fs::write(&store_auth, r#"{"token":"STORE_TIE"}"#).unwrap();
    set_mtime(&store_auth, t0);

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .expect("sync_codex_stage_to_store must succeed on an mtime tie");

    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"STORE_TIE"}"#,
        "an equal-mtime stage auth.json must not overwrite the store"
    );
}

#[cfg(unix)]
#[test]
fn sync_codex_stage_to_store_rejects_symlink_staged_auth() {
    // A container with the rw-mounted stage could swap the staged auth.json
    // for a symlink pointing at an arbitrary host path just before the
    // container stops, hoping the run-after sync follows it and either leaks
    // an arbitrary host file into the store or corrupts the store via a
    // dangling/malicious target. `remove_dst_if_not_regular_file` (the same
    // defense `prepare_codex_mount` uses, round 5/8) must reject this: detect
    // via `symlink_metadata` (never follows), delete without reading through
    // it, and leave the store untouched.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");
    let store_content_before = fs::read_to_string(&store_auth).unwrap();

    let victim = home_dir.path().join("victim.txt");
    fs::write(&victim, "VICTIM_UNCHANGED").unwrap();
    fs::remove_file(&staged_auth).unwrap();
    symlink(&victim, &staged_auth).unwrap();

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .expect(
        "sync_codex_stage_to_store must not error on a tampered stage; it must reject and \
         continue rather than fail the whole run's cleanup",
    );

    assert!(
        !staged_auth.exists(),
        "the tampered symlink must be deleted, not left in place or followed"
    );
    assert_eq!(
        fs::read_to_string(&victim).unwrap(),
        "VICTIM_UNCHANGED",
        "the symlink target must never be read or written to"
    );
    let store_content_after = fs::read_to_string(&store_auth).unwrap();
    assert_eq!(
        store_content_after, store_content_before,
        "the auth store must be left completely untouched when the stage auth.json was a symlink"
    );
    // Belt-and-suspenders: assert directly (not just via equality with the
    // pre-tamper snapshot) that the symlink target's content never reached
    // the store. round 10 differs from earlier TOCTOU defenses in that the
    // *source* of the sync (the stage) is the untrusted, container-writable
    // side, not the destination -- an implementation that opened the
    // symlink's target by path instead of rejecting it outright could
    // ingest the victim file's content into the store.
    assert_ne!(
        store_content_after, "VICTIM_UNCHANGED",
        "the symlink target's content must never be ingested into the auth store"
    );
}

#[cfg(unix)]
#[test]
fn sync_codex_stage_to_store_rejects_directory_staged_auth() {
    // Same defense as the symlink case, but for a directory masquerading as
    // `auth.json` (round 8 P1's non-regular-file defense). A directory's
    // mtime can easily look "newer" than the store's real auth.json, which
    // (absent this defense) would otherwise pass the mtime check and corrupt
    // the store on copy.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");
    let store_content_before = fs::read_to_string(&store_auth).unwrap();

    fs::remove_file(&staged_auth).unwrap();
    fs::create_dir_all(&staged_auth).unwrap();
    fs::write(staged_auth.join("junk"), "SECRET_JUNK_INSIDE_TAMPERED_DIR").unwrap();

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .expect("sync_codex_stage_to_store must not error on a tampered stage");

    assert!(
        !staged_auth.exists(),
        "the directory masquerading as auth.json must be deleted (remove_dir_all), not left \
         in place"
    );
    let store_content_after = fs::read_to_string(&store_auth).unwrap();
    assert_eq!(
        store_content_after, store_content_before,
        "the auth store must be left completely untouched when the stage auth.json was a \
         directory"
    );
    assert!(
        !store_content_after.contains("SECRET_JUNK_INSIDE_TAMPERED_DIR"),
        "content from inside the tampered directory must never reach the auth store"
    );
}

#[cfg(unix)]
#[test]
fn sync_codex_stage_to_store_ingests_and_normalizes_permissions_of_a_loosely_permissioned_stage_auth(
) {
    // Per spec (docs/specs/v1.8/2026-07-21-codex-in-container.md round 10,
    // lines 393-394): a loosely-permissioned *regular* file is NOT type
    // tampering, so it is not rejected — only symlink/directory type swaps
    // are. `remove_dst_if_not_regular_file` inspects file *type* only (via
    // `symlink_metadata`), never permission bits, so a 0644 regular file is
    // ingested on the normal mtime comparison just like a 0600 file. The
    // store copy is then always normalized to 0600 — `copy_codex_asset_
    // atomically` sets 0600 on its temp file before persisting, and
    // `enforce_staged_permissions` re-asserts it — so a loosened source
    // permission never survives into the store. This test asserts exactly
    // that: ingested, and normalized to 0600.
    use std::os::unix::fs::PermissionsExt;

    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    let t0 = SystemTime::now();
    set_mtime(&store_auth, t0);
    fs::write(
        &staged_auth,
        r#"{"token":"CONTAINER_REFRESHED_LOOSE_PERMS"}"#,
    )
    .unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(60));
    fs::set_permissions(&staged_auth, fs::Permissions::from_mode(0o644)).unwrap();

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED_LOOSE_PERMS"}"#,
        "a newer stage auth.json is ingested regardless of its own permission bits"
    );
    let mode = fs::metadata(&store_auth).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the store's copy must always end up 0600 regardless of the stage's permission bits, \
         got {:o}",
        mode
    );
}

// --- sync_codex_stage_to_store: run-after JSON-integrity gate on the
// still-running path (codex review round 11 P1) ---
//
// When the container keeps running (interactive's reused container), the
// rw-mounted stage auth.json can be observed mid-write. A torn/partial write is
// not valid JSON; ingesting it would persist a corrupted token into the store,
// which is distributed to every container. `ContainerLiveness::Running` must
// gate the ingest on a successful `serde_json` parse and leave the store
// untouched on failure. `ContainerLiveness::Stopped` (the container already
// stopped/removed, so no concurrent write is possible) deliberately skips that
// gate — the liveness argument is the type-level enforcement of the ordering
// guarantee, which these tests exercise directly.

#[test]
fn sync_codex_stage_to_store_skips_invalid_json_stage_when_running_and_keeps_store() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    // The store already holds a valid auth.json (from prepare_codex_mount's
    // host -> store hop). Pin its mtime so we can assert it is not rewritten.
    let store_content_before = fs::read_to_string(&store_auth).unwrap();
    let t0 = SystemTime::now();
    set_mtime(&store_auth, t0);
    let store_mtime_before = fs::metadata(&store_auth).unwrap().modified().unwrap();

    // The container is still writing a token refresh: the stage auth.json is a
    // strictly newer *but truncated* (mid-write) JSON object that would
    // normally be promoted by keep-newest.
    fs::write(&staged_auth, r#"{"token":"CONTAINER_REFRE"#).unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Running,
    )
    .expect("sync must not fail the run's cleanup just because the snapshot was mid-write");

    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        store_content_before,
        "a mid-write (invalid JSON) stage auth.json must never overwrite the store when Running"
    );
    let store_mtime_after = fs::metadata(&store_auth).unwrap().modified().unwrap();
    assert_eq!(
        store_mtime_before, store_mtime_after,
        "the store's auth.json must not even be rewritten (mtime unchanged) on a skipped sync"
    );
}

#[test]
fn sync_codex_stage_to_store_skips_invalid_json_stage_when_running_and_leaves_empty_store() {
    // Same integrity gate, but the store is empty (no prior valid auth). A
    // mid-write stage must not cause a bogus store auth.json to be created.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    // Empty the store so `should_copy` is unconditionally true; the only thing
    // that can then stop the ingest is the JSON-integrity gate itself.
    fs::remove_file(&store_auth).unwrap();

    fs::write(&staged_auth, r#"{"token":"CONTAINER_REFRE"#).unwrap();
    set_mtime(&staged_auth, SystemTime::now());

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Running,
    )
    .expect("sync must not fail on a mid-write snapshot even with an empty store");

    assert!(
        !store_auth.exists(),
        "a mid-write (invalid JSON) stage must not create a store auth.json out of nothing"
    );
}

#[test]
fn sync_codex_stage_to_store_promotes_valid_json_stage_when_running() {
    // Valid JSON passes the Running integrity gate, so the normal keep-newest
    // promotion still happens (a strictly newer stage wins) — the gate only
    // rejects torn writes, it does not change behavior for well-formed tokens.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    let t0 = SystemTime::now();
    set_mtime(&store_auth, t0);
    fs::write(&staged_auth, r#"{"token":"CONTAINER_REFRESHED_VALID"}"#).unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Running,
    )
    .expect("a well-formed, newer stage auth.json must sync successfully when Running");

    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"CONTAINER_REFRESHED_VALID"}"#,
        "a valid, newer stage auth.json must be promoted into the store even on the Running path"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&store_auth).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "the promoted auth.json in the store must be 0600, got {:o}",
            mode
        );
    }
}

#[test]
fn sync_codex_stage_to_store_copies_invalid_json_stage_when_stopped() {
    // The liveness argument is what switches behavior. `Stopped` means the
    // container has already stopped/removed (docker stop/rm -f returned), so no
    // torn write is possible and the JSON-integrity gate is deliberately
    // skipped: the very same invalid-JSON bytes that `Running` rejects are
    // copied through verbatim under `Stopped`. This is the type-level proof
    // that the liveness argument enforces the ordering guarantee (stop-then-
    // sync) rather than relying on unobservable process timing.
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    make_host_codex(home_dir.path());

    let dir = prepare_codex_mount(home_dir.path(), config_dir.path(), runtime_dir.path())
        .unwrap()
        .expect("first run should stage auth.json");
    let staged_auth = dir.join("auth.json");
    let store_auth = config_dir.path().join("codex-auth").join("auth.json");

    let t0 = SystemTime::now();
    set_mtime(&store_auth, t0);
    // Byte-for-byte the same truncated payload the Running tests reject.
    fs::write(&staged_auth, r#"{"token":"NOT_VALID_JSON_BUT_STOPPED"#).unwrap();
    set_mtime(&staged_auth, t0 + Duration::from_secs(3_600));

    sync_codex_stage_to_store(
        runtime_dir.path(),
        config_dir.path(),
        ContainerLiveness::Stopped,
    )
    .expect("Stopped sync must succeed and copy without a JSON-integrity check");

    assert_eq!(
        fs::read_to_string(&store_auth).unwrap(),
        r#"{"token":"NOT_VALID_JSON_BUT_STOPPED"#,
        "under Stopped the integrity gate is skipped, so a newer stage is copied verbatim"
    );
}
