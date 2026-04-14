//! `--mode review --prompt ...` must NOT pass --dangerously-skip-permissions
//! to Claude. Impl mode autonomous runs must still bypass (no user to approve).
//!
//! Signal: VIBEPOD_TRACE=1 dumps claude_args to stderr right after they're
//! built in prepare.rs, before downstream (docker) failures. We assert on
//! that trace line.

use std::path::Path;
use std::process::Command;

fn init_git(dir: &Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .expect("git");
        assert!(status.success(), "git {:?} failed", args);
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["commit", "--allow-empty", "-q", "-m", "init"]);
}

/// Seed enough state (config.toml + ecc cache + extracted template) so
/// prepare_context reaches the claude_args build step where VIBEPOD_TRACE
/// dumps the effective args.
fn seed_env(home: &Path, template_rel: &str) {
    let config_dir = home.join(".config/vibepod");
    std::fs::create_dir_all(&config_dir).unwrap();
    // Minimal config.toml to pass load_global_config.
    std::fs::write(
        config_dir.join("config.toml"),
        "[global]\ndefault_agent = \"claude\"\nimage = \"vibepod-claude:latest\"\n",
    )
    .unwrap();
    // Pre-seed ecc-cache so the fail-fast check passes.
    std::fs::create_dir_all(config_dir.join("ecc-cache/.git")).unwrap();
    std::fs::write(
        config_dir.join("ecc-cache/.git/HEAD"),
        "ref: refs/heads/main\n",
    )
    .unwrap();
    // Pre-extract the template so resolve_template_dir passes.
    let tmpl_dir = config_dir.join("templates").join(template_rel);
    std::fs::create_dir_all(&tmpl_dir).unwrap();
    std::fs::write(tmpl_dir.join(".vibepod-embedded"), "test\n").unwrap();
    std::fs::write(tmpl_dir.join("CLAUDE.md"), "test\n").unwrap();
    std::fs::write(
        tmpl_dir.join("vibepod-template.toml"),
        "[runtime]\nrequired_langs = []\n",
    )
    .unwrap();
}

#[test]
fn review_mode_autonomous_does_not_bypass_permissions() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    seed_env(tmp.path(), "generic/review");

    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--mode", "review", "--prompt", "x"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);

    // The claude_args trace must be present for this test to mean
    // anything (otherwise the negative assertion is vacuous).
    assert!(
        stderr.contains("claude_args ="),
        "expected VIBEPOD_TRACE claude_args dump; stderr:\n{stderr}"
    );
    // And the flag must NOT be in that args vector.
    let args_line = stderr
        .lines()
        .find(|l| l.contains("claude_args ="))
        .unwrap_or("");
    assert!(
        !args_line.contains("--dangerously-skip-permissions"),
        "review mode must not bypass permissions; args line:\n{args_line}"
    );
}

#[test]
fn impl_mode_autonomous_still_bypasses_permissions() {
    let tmp = tempfile::tempdir().unwrap();
    init_git(tmp.path());
    // No lang / no mode / no template → <host> path; no ecc/template
    // seeding needed, but we do need config.toml for load_global_config.
    let config_dir = tmp.path().join(".config/vibepod");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "[global]\ndefault_agent = \"claude\"\nimage = \"vibepod-claude:latest\"\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_vibepod"))
        .current_dir(tmp.path())
        .args(["run", "--prompt", "x"]) // impl is default
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("VIBEPOD_TRACE", "1")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);

    let args_line = stderr
        .lines()
        .find(|l| l.contains("claude_args ="))
        .unwrap_or("");
    assert!(
        args_line.contains("--dangerously-skip-permissions"),
        "impl mode autonomous must bypass permissions; stderr:\n{stderr}"
    );
}
