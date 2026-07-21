//! Host `~/.claude/` assets reaching the container.
//!
//! Two rules are under test:
//!
//! 1. **Allowlist** — only CLAUDE.md / agents / skills / specs travel into
//!    the container. Session and history data must never be copied, both
//!    because it is large and because it contains other projects'
//!    conversations.
//! 2. **Template wins on collision** — a template defines mode-specific
//!    behaviour (notably `review` mode's `permissions.deny`), so no host
//!    file may shadow a template file of the same name. Host assets are
//!    additive only.

use std::fs;
use std::path::Path;

use vibepod::cli::run::{
    build_claude_config_mounts, host_claude_stage_entries, merge_claude_md,
    merge_host_plugins_mounts,
    prepare::{assemble_staging, should_reassemble_staging},
};

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

// --- Staging merge: template mode ---

/// Assemble staging from a template dir plus the host home, with no ecc
/// section involved.
fn stage(
    home: &Path,
    build_template: impl FnOnce(&Path),
) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    let runtime_dir = tmp.path().join("runtime");
    let template_dir = tmp.path().join("template");
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&template_dir).unwrap();
    build_template(&template_dir);

    let staging = assemble_staging(
        &config_dir,
        &runtime_dir,
        &template_dir,
        home,
        "test-template",
    )
    .unwrap();
    (tmp, staging)
}

#[test]
fn template_mode_carries_host_assets_into_staging() {
    // The core of requirement 1: template mode used to ignore ~/.claude/
    // entirely.
    let (_home_tmp, home) = setup_home();

    let (_tmp, staging) = stage(&home, |t| {
        fs::write(t.join("settings.json"), r#"{"permissions":{"deny":[]}}"#).unwrap();
    });

    assert_eq!(
        fs::read_to_string(staging.join("CLAUDE.md")).unwrap(),
        "HOST_RULES"
    );
    assert_eq!(
        fs::read_to_string(staging.join("skills/host-skill/SKILL.md")).unwrap(),
        "HOST_SKILL"
    );
    assert_eq!(
        fs::read_to_string(staging.join("agents/host-agent.md")).unwrap(),
        "HOST_AGENT"
    );
    assert_eq!(
        fs::read_to_string(staging.join("specs/security-rules.md")).unwrap(),
        "HOST_SPEC"
    );
}

#[test]
fn template_claude_md_is_merged_above_host_claude_md() {
    // CLAUDE.md is not a plain shadow like other files: template and host
    // are concatenated so the host's personal instructions survive, with the
    // template placed first (it takes precedence on conflict) and the host
    // below a labeled separator.
    let (_home_tmp, home) = setup_home();

    let (_tmp, staging) = stage(&home, |t| {
        fs::write(t.join("CLAUDE.md"), "TEMPLATE_RULES").unwrap();
    });

    let merged = fs::read_to_string(staging.join("CLAUDE.md")).unwrap();
    assert!(
        merged.contains("TEMPLATE_RULES"),
        "template rules must be present, got: {merged}"
    );
    assert!(
        merged.contains("HOST_RULES"),
        "host rules must survive the merge, got: {merged}"
    );
    let t_pos = merged.find("TEMPLATE_RULES").unwrap();
    let h_pos = merged.find("HOST_RULES").unwrap();
    assert!(
        t_pos < h_pos,
        "template content must come before host content, got: {merged}"
    );
}

#[test]
fn host_and_template_skills_merge_per_file_rather_than_per_directory() {
    // A bind mount can only pick one directory wholesale. Staging exists so
    // that a template contributing one skill does not erase the host's
    // other skills — while still winning on the one name they share.
    let (_home_tmp, home) = setup_home();

    let (_tmp, staging) = stage(&home, |t| {
        fs::create_dir_all(t.join("skills/host-skill")).unwrap();
        fs::write(t.join("skills/host-skill/SKILL.md"), "TEMPLATE_SKILL").unwrap();
        fs::create_dir_all(t.join("skills/template-only")).unwrap();
        fs::write(t.join("skills/template-only/SKILL.md"), "TEMPLATE_ONLY").unwrap();
    });

    // Collision: template wins.
    assert_eq!(
        fs::read_to_string(staging.join("skills/host-skill/SKILL.md")).unwrap(),
        "TEMPLATE_SKILL"
    );
    // Non-colliding template skill survives.
    assert_eq!(
        fs::read_to_string(staging.join("skills/template-only/SKILL.md")).unwrap(),
        "TEMPLATE_ONLY"
    );
    // Non-colliding host asset in a *different* allowlisted dir survives.
    assert_eq!(
        fs::read_to_string(staging.join("agents/host-agent.md")).unwrap(),
        "HOST_AGENT"
    );
}

#[test]
fn host_settings_json_never_reaches_staging() {
    // The safety argument for --dangerously-skip-permissions in review mode
    // is the template's permissions.deny. A host settings.json must not be
    // able to influence it — not even when the template ships none.
    let (_home_tmp, home) = setup_home();

    let (_tmp, staging) = stage(&home, |t| {
        fs::write(t.join("CLAUDE.md"), "T").unwrap();
    });

    assert!(
        !staging.join("settings.json").exists(),
        "host settings.json must never be staged"
    );
}

#[test]
fn host_session_and_history_data_never_reaches_staging() {
    let (_home_tmp, home) = setup_home();

    let (_tmp, staging) = stage(&home, |t| {
        fs::write(t.join("CLAUDE.md"), "T").unwrap();
    });

    for forbidden in [
        "sessions",
        "projects",
        "todos",
        "shell-snapshots",
        "file-history",
        "backups",
        "history.jsonl",
    ] {
        assert!(
            !staging.join(forbidden).exists(),
            "{} must never be staged into the container",
            forbidden
        );
    }
}

#[test]
fn symlinked_host_skill_is_skipped_without_failing_the_run() {
    // Unlike a template (third-party, so a symlink aborts), ~/.claude/ is
    // the user's own directory where symlinked skills are routine.
    // Aborting there would make vibepod unusable for those users; following
    // the link would silently pull outside content into the container.
    let (_home_tmp, home) = setup_home();
    let outside = home.join("outside-skill");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("SKILL.md"), "OUTSIDE").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, home.join(".claude/skills/linked")).unwrap();

    let (_tmp, staging) = stage(&home, |t| {
        fs::write(t.join("CLAUDE.md"), "T").unwrap();
    });

    // The run succeeded, non-symlinked assets are intact...
    assert_eq!(
        fs::read_to_string(staging.join("skills/host-skill/SKILL.md")).unwrap(),
        "HOST_SKILL"
    );
    // ...and the symlink target was not materialized into staging.
    #[cfg(unix)]
    assert!(
        !staging.join("skills/linked").exists(),
        "symlinked host skill must not be copied into staging"
    );
}

// --- Host plugins merge (template mode) ---

#[test]
fn host_plugins_are_added_when_the_template_defines_none() {
    let home = Path::new("/Users/alice");
    let mut mounts = vec![(
        "/staging/CLAUDE.md".to_string(),
        "/home/vibepod/.claude/CLAUDE.md".to_string(),
    )];

    merge_host_plugins_mounts(&mut mounts, Some("/Users/alice/.claude/plugins"), home);

    // Double mount: the container-visible path plus the host absolute path
    // that installed_plugins.json's installPath entries point at.
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/plugins"));
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/Users/alice/.claude/plugins"));
}

#[test]
fn template_plugins_suppress_host_plugins_entirely() {
    // Both would target /home/vibepod/.claude/plugins; docker rejects a
    // duplicate destination, and the template's plugin set is authoritative.
    let home = Path::new("/Users/alice");
    let mut mounts = vec![(
        "/staging/plugins".to_string(),
        "/home/vibepod/.claude/plugins".to_string(),
    )];

    merge_host_plugins_mounts(&mut mounts, Some("/Users/alice/.claude/plugins"), home);

    assert_eq!(
        mounts.len(),
        1,
        "host plugins must not be added when the template owns plugins: {:?}",
        mounts
    );
    assert_eq!(mounts[0].0, "/staging/plugins");
}

#[test]
fn absent_host_plugins_add_nothing() {
    let home = Path::new("/Users/alice");
    let mut mounts = vec![(
        "/staging/CLAUDE.md".to_string(),
        "/home/vibepod/.claude/CLAUDE.md".to_string(),
    )];

    merge_host_plugins_mounts(&mut mounts, None, home);

    assert_eq!(mounts.len(), 1);
}

// --- merge_claude_md: the four presence combinations ---
//
// Every official bundle ships a CLAUDE.md, so a plain per-file overwrite
// would always drop the host's. These pin the concatenation contract:
// template first (precedence preserved), host second, both preserved.

#[test]
fn merge_claude_md_both_absent_yields_none() {
    // Nothing to write — assemble_staging leaves no CLAUDE.md at all.
    assert_eq!(merge_claude_md(None, None, "rust/impl"), None);
}

#[test]
fn merge_claude_md_template_only_returns_template_verbatim() {
    // Custom template with a CLAUDE.md, host has none: template stands alone.
    assert_eq!(
        merge_claude_md(Some("TEMPLATE RULES"), None, "rust/impl"),
        Some("TEMPLATE RULES".to_string())
    );
}

#[test]
fn merge_claude_md_host_only_returns_host_verbatim() {
    // Custom template without a CLAUDE.md, host has one: host stands alone,
    // no separator header is introduced.
    let merged = merge_claude_md(None, Some("HOST RULES"), "blank");
    assert_eq!(merged, Some("HOST RULES".to_string()));
}

#[test]
fn merge_claude_md_both_present_concatenates_template_before_host() {
    // The load-bearing case: both survive, template body precedes host body,
    // and the separator names each section's provenance.
    let merged =
        merge_claude_md(Some("TEMPLATE RULES"), Some("HOST RULES"), "rust/review").unwrap();

    let t = merged
        .find("TEMPLATE RULES")
        .expect("template body present");
    let h = merged.find("HOST RULES").expect("host body present");
    assert!(t < h, "template must come before host:\n{merged}");
    assert!(
        merged.contains("template 'rust/review'"),
        "separator must name the template:\n{merged}"
    );
    assert!(
        merged.contains("host ~/.claude/CLAUDE.md"),
        "separator must name the host source:\n{merged}"
    );
}

// (End-to-end staging coverage for the merge already lives in
// `template_claude_md_is_merged_above_host_claude_md`; the four cases above
// pin the pure function itself.)

// --- should_reassemble_staging ---
//
// Reassembly wipes and rebuilds the staging dir, which is the bind-mount
// SOURCE for a persistent container's ~/.claude/. These pin when that wipe
// is safe.

#[test]
fn reassemble_when_reusing_a_running_container_is_skipped() {
    // Wiping live staging under a Running container we are about to reuse
    // would momentarily empty its mount — so skip.
    assert!(!should_reassemble_staging(true, true));
}

#[test]
fn reassemble_for_a_non_running_container_proceeds() {
    // Stopped / new container: safe (and necessary) to rebuild.
    assert!(should_reassemble_staging(false, true));
}

#[test]
fn reassemble_always_runs_when_no_staging_exists_yet() {
    // Nothing to reuse; downstream mount-building needs the dir to exist.
    // True regardless of the running-reuse flag.
    assert!(should_reassemble_staging(true, false));
    assert!(should_reassemble_staging(false, false));
}

// --- top-level symlink policy ---

#[test]
#[cfg(unix)]
fn top_level_symlinked_skills_dir_is_followed_into_staging() {
    // Two-tier symlink policy: a whole ~/.claude/skills symlinked elsewhere
    // (a common dotfiles setup, an explicit user placement) is FOLLOWED,
    // unlike a symlink nested inside it. This fixes that top-level behaviour
    // so it cannot silently regress to the nested-skip rule.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/CLAUDE.md"), "R").unwrap();

    // Real skills dir living outside ~/.claude, linked in as the whole dir.
    let real_skills = tmp.path().join("dotfiles/skills");
    fs::create_dir_all(real_skills.join("linked-skill")).unwrap();
    fs::write(real_skills.join("linked-skill/SKILL.md"), "LINKED").unwrap();
    std::os::unix::fs::symlink(&real_skills, home.join(".claude/skills")).unwrap();

    // host_claude_stage_entries follows the top-level symlink (is_dir()).
    let names: Vec<&str> = host_claude_stage_entries(&home.join(".claude"))
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert!(
        names.contains(&"skills"),
        "top-level symlinked skills dir should be seen: {names:?}"
    );

    // ...and its contents materialize in staging.
    let (_tmp, staging) = stage(&home, |t| {
        fs::write(t.join("CLAUDE.md"), "T").unwrap();
    });
    assert_eq!(
        fs::read_to_string(staging.join("skills/linked-skill/SKILL.md")).unwrap(),
        "LINKED",
        "contents of a top-level symlinked skills dir must be followed into staging"
    );
}
