//! When `--lang rust` is passed, the rust/impl template should be selected.
//! Verified via VIBEPOD_TRACE=1 stderr output, which precedes downstream
//! (docker/config) failures.

use std::path::Path;
use std::process::Command;

/// Initialize a minimal git repo in `dir` so `prepare_context`'s
/// `is_git_repo` check passes and execution proceeds to the template
/// resolver (which is what we're testing). The trace is emitted *after*
/// the final `or_else` resolution step, so downstream steps like git /
/// docker must not short-circuit the run before that point.
fn init_git(dir: &Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            // Ensure user.name/user.email are defined for the commit below
            // without depending on the host's global git config.
            .env("GIT_AUTHOR_NAME", "vibepod-test")
            .env("GIT_AUTHOR_EMAIL", "test@vibepod.invalid")
            .env("GIT_COMMITTER_NAME", "vibepod-test")
            .env("GIT_COMMITTER_EMAIL", "test@vibepod.invalid")
            .status()
            .expect("git");
        assert!(
            status.success(),
            "git {:?} failed in {}",
            args,
            dir.display()
        );
    };
    run(&["init", "-q", "-b", "main"]);
    // An initial commit is required because prepare_context reads HEAD.
    run(&["commit", "--allow-empty", "-q", "-m", "init"]);
}

/// Pre-seed a fake ecc-cache `.git` dir so the Task 14 fail-fast check
/// does not bail before we reach the VIBEPOD_TRACE emission. The trace
/// is what we assert on; the real clone is not needed for these tests.
fn seed_ecc_cache(home: &Path) {
    let cache_git = home.join(".config/vibepod/ecc-cache/.git");
    std::fs::create_dir_all(&cache_git).expect("mkdir ecc-cache/.git");
}

#[test]
fn lang_rust_routes_to_rust_impl() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    seed_ecc_cache(tmp.path());
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
    init_git(tmp.path());
    seed_ecc_cache(tmp.path());
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
    init_git(tmp.path());
    seed_ecc_cache(tmp.path());
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
    init_git(tmp.path());
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

#[test]
fn explicit_lang_wins_over_cwd_detect() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    seed_ecc_cache(tmp.path());
    // Write a Cargo.toml so cwd-detect would pick rust,
    // but pass --lang go to prove explicit flag takes priority.
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"fake\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--lang", "go", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("selected template = go/impl"),
        "explicit --lang go must win over cwd Cargo.toml, stderr:\n{stderr}"
    );
}

/// Regression: v1.6 introduced nested official-bundle names like
/// `rust/impl`, but the template-name validator only accepted flat names
/// (ASCII alnum + `-_`). `--lang rust` therefore hard-failed with
/// `Template name 'rust/impl' is invalid` *before* Docker was ever
/// touched, making the feature unusable end-to-end even though the
/// existing `VIBEPOD_TRACE` tests (which assert only on trace output,
/// emitted *before* validation runs) kept passing.
///
/// This test exercises the resolver past trace emission, past template
/// validation, so the old code fails this assertion while the fixed
/// validator makes it pass (even though the run still errors out
/// downstream for other reasons, such as Docker not being available).
#[test]
fn rust_impl_template_name_passes_validation() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    seed_ecc_cache(tmp.path());
    // Pre-extract a minimal `templates/rust/impl/` so resolve_template_dir
    // passes without needing Docker / the real ecc cache. This simulates
    // the state after `vibepod init` + lazy extract, so validation is the
    // only remaining gate the test is actually asserting on.
    let tmpl_dir = tmp.path().join(".config/vibepod/templates/rust/impl");
    std::fs::create_dir_all(&tmpl_dir).unwrap();
    std::fs::write(tmpl_dir.join(".vibepod-embedded"), "test\n").unwrap();
    std::fs::write(tmpl_dir.join("CLAUDE.md"), "test\n").unwrap();
    std::fs::write(
        tmpl_dir.join("vibepod-template.toml"),
        "[runtime]\nrequired_langs = [\"rust\"]\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--lang", "rust", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !combined.contains("Template name 'rust/impl' is invalid"),
        "regression: --lang rust hits template-name validation. combined output:\n{combined}"
    );
    // The run itself is expected to still fail (Docker not available in
    // CI, or downstream ecc/stage steps), but NOT with the nested-name
    // validation error — that's the whole point of this test.
}

#[test]
fn unsupported_lang_falls_through_to_host() {
    // Unknown lang like "fortran" is not an error — it soft-falls through
    // to <host>. (If we ever decide to harden this into an error, this
    // test documents the transition point.)
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--lang", "fortran", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("selected template = <host>"),
        "unsupported lang must soft-fallthrough to <host>, stderr:\n{stderr}"
    );
}
