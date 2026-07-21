//! Host `~/.claude/` assets reaching the container (host mode).
//!
//! The invariant under test: only an allowlisted subset of `~/.claude/`
//! (CLAUDE.md / agents / skills / specs, plus plugins) is mounted into the
//! container. Session and history data must never travel in, both because it
//! is large and because it contains other projects' conversations.

use std::fs;
use std::path::Path;

use vibepod::cli::run::{build_claude_config_mounts, host_claude_stage_entries};

/// Build a host `~/.claude/` containing both allowlisted assets and the
/// history/session data that must be left behind.
fn make_host_claude(home: &Path) {
    let claude = home.join(".claude");

    // Allowlisted
    fs::write(claude.join("CLAUDE.md"), "HOST_RULES").unwrap();
    fs::create_dir_all(claude.join("skills/host-skill")).unwrap();
    fs::write(claude.join("skills/host-skill/SKILL.md"), "HOST_SKILL").unwrap();
    fs::create_dir_all(claude.join("agents")).unwrap();
    fs::write(claude.join("agents/host-agent.md"), "HOST_AGENT").unwrap();
    fs::create_dir_all(claude.join("specs")).unwrap();
    fs::write(claude.join("specs/security-rules.md"), "HOST_SPEC").unwrap();

    // Must never be carried into the container
    fs::create_dir_all(claude.join("sessions")).unwrap();
    fs::write(claude.join("sessions/s1.jsonl"), "SECRET_SESSION").unwrap();
    fs::create_dir_all(claude.join("projects/other-project")).unwrap();
    fs::write(claude.join("projects/other-project/x.json"), "SECRET_PROJ").unwrap();
    fs::create_dir_all(claude.join("todos")).unwrap();
    fs::write(claude.join("todos/t.json"), "SECRET_TODO").unwrap();
    fs::create_dir_all(claude.join("shell-snapshots")).unwrap();
    fs::write(claude.join("shell-snapshots/snap.sh"), "SECRET_SNAP").unwrap();
    fs::create_dir_all(claude.join("file-history")).unwrap();
    fs::create_dir_all(claude.join("backups")).unwrap();
    fs::write(claude.join("history.jsonl"), "SECRET_HISTORY").unwrap();
    fs::write(claude.join("settings.json"), r#"{"permissions":{}}"#).unwrap();
}

fn setup_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    make_host_claude(&home);
    (tmp, home)
}

// --- Allowlist ---

#[test]
fn stage_entries_selects_only_allowlisted_assets() {
    let (_tmp, home) = setup_home();

    let names: Vec<&str> = host_claude_stage_entries(&home.join(".claude"))
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert_eq!(names, vec!["CLAUDE.md", "agents", "skills", "specs"]);
}

#[test]
fn stage_entries_skips_absent_assets_without_error() {
    // A host with no ~/.claude/specs is the normal case, not an error.
    let tmp = tempfile::tempdir().unwrap();
    let claude = tmp.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(claude.join("CLAUDE.md"), "R").unwrap();

    let names: Vec<&str> = host_claude_stage_entries(&claude)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert_eq!(names, vec!["CLAUDE.md"]);
}

#[test]
fn host_mode_mounts_include_specs() {
    // `specs/` was previously never mounted in any mode.
    let (_tmp, home) = setup_home();

    let mounts = build_claude_config_mounts(&home);

    assert!(
        mounts
            .iter()
            .any(|(_, dst)| dst == "/home/vibepod/.claude/specs"),
        "expected ~/.claude/specs to be mounted, got {:?}",
        mounts
    );
}

#[test]
fn host_mode_mounts_exclude_session_and_history_data() {
    let (_tmp, home) = setup_home();

    let mounts = build_claude_config_mounts(&home);

    for forbidden in ["sessions", "projects", "todos", "history.jsonl", "backups"] {
        assert!(
            !mounts.iter().any(|(src, _)| src.contains(forbidden)),
            "{} must never be mounted, got {:?}",
            forbidden,
            mounts
        );
    }
}

// --- top-level symlink policy ---

#[test]
#[cfg(unix)]
fn top_level_symlinked_skills_dir_is_followed() {
    // A whole ~/.claude/skills symlinked elsewhere (a common dotfiles setup,
    // an explicit user placement) is FOLLOWED: host_claude_stage_entries uses
    // is_dir(), which resolves the top-level symlink. This pins that behaviour
    // so it cannot silently regress.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/CLAUDE.md"), "R").unwrap();

    // Real skills dir living outside ~/.claude, linked in as the whole dir.
    let real_skills = tmp.path().join("dotfiles/skills");
    fs::create_dir_all(real_skills.join("linked-skill")).unwrap();
    fs::write(real_skills.join("linked-skill/SKILL.md"), "LINKED").unwrap();
    std::os::unix::fs::symlink(&real_skills, home.join(".claude/skills")).unwrap();

    let names: Vec<&str> = host_claude_stage_entries(&home.join(".claude"))
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert!(
        names.contains(&"skills"),
        "top-level symlinked skills dir should be seen: {names:?}"
    );
}
