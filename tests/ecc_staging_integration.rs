//! End-to-end staging: embedded template + ecc-cache → assembled staging.

use std::fs;

#[test]
fn staging_combines_embedded_and_ecc_files() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    let runtime_dir = tmp.path().join("runtime");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&config_dir).unwrap();

    // Fake ecc cache with one skill + one agent
    let cache = config_dir.join("ecc-cache");
    fs::create_dir_all(cache.join(".git")).unwrap();
    fs::create_dir_all(cache.join("skills/rust-patterns")).unwrap();
    fs::write(cache.join("skills/rust-patterns/SKILL.md"), "SK").unwrap();
    fs::create_dir_all(cache.join("agents")).unwrap();
    fs::write(cache.join("agents/rust-reviewer.md"), "AG").unwrap();

    // Fake extracted template dir with CLAUDE.md + vibepod-template.toml with [ecc]
    let template_dir = runtime_dir.join("template");
    fs::create_dir_all(&template_dir).unwrap();
    fs::write(template_dir.join("CLAUDE.md"), "RULES").unwrap();
    fs::write(template_dir.join("settings.json"), "{}").unwrap();
    fs::write(
        template_dir.join("vibepod-template.toml"),
        r#"
[ecc]
skills = ["skills/rust-patterns/SKILL.md"]
agents = ["agents/rust-reviewer.md"]
"#,
    )
    .unwrap();

    let staging =
        vibepod::cli::run::prepare::assemble_staging(&config_dir, &runtime_dir, &template_dir)
            .unwrap();

    // Files from template_dir should be copied as-is
    assert!(staging.join("CLAUDE.md").is_file());
    assert_eq!(
        fs::read_to_string(staging.join("CLAUDE.md")).unwrap(),
        "RULES"
    );
    assert!(staging.join("settings.json").is_file());
    assert!(staging.join("vibepod-template.toml").is_file());

    // [ecc] selection should be staged at top-level (matches mount wiring
    // that maps `<staging>/skills` → `/home/vibepod/.claude/skills`).
    assert!(staging.join("skills/rust-patterns/SKILL.md").is_file());
    assert!(staging.join("agents/rust-reviewer.md").is_file());
}

#[test]
fn assemble_staging_works_without_ecc_section() {
    // Template without [ecc] — staging is just a copy of template_dir contents.
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    let runtime_dir = tmp.path().join("runtime");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&config_dir).unwrap();

    let template_dir = runtime_dir.join("template");
    fs::create_dir_all(&template_dir).unwrap();
    fs::write(template_dir.join("CLAUDE.md"), "plain").unwrap();
    // No vibepod-template.toml — skip ecc step entirely

    let staging =
        vibepod::cli::run::prepare::assemble_staging(&config_dir, &runtime_dir, &template_dir)
            .unwrap();
    assert!(staging.join("CLAUDE.md").is_file());
    // No [ecc] section means no skills/agents should be staged.
    assert!(!staging.join("skills").exists());
    assert!(!staging.join("agents").exists());
}

#[test]
fn assemble_staging_fails_fast_on_missing_ecc_file() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    let runtime_dir = tmp.path().join("runtime");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(config_dir.join("ecc-cache/.git")).unwrap();
    // Intentionally do NOT create the skill file in cache.

    let template_dir = runtime_dir.join("template");
    fs::create_dir_all(&template_dir).unwrap();
    fs::write(
        template_dir.join("vibepod-template.toml"),
        r#"
[ecc]
skills = ["skills/ghost/SKILL.md"]
"#,
    )
    .unwrap();

    let err =
        vibepod::cli::run::prepare::assemble_staging(&config_dir, &runtime_dir, &template_dir)
            .unwrap_err();
    assert!(
        format!("{err}").contains("skills/ghost"),
        "expected missing-file error, got: {err}"
    );
}

#[cfg(unix)]
#[test]
fn assemble_staging_rejects_symlink_in_template_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    let runtime_dir = tmp.path().join("runtime");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&config_dir).unwrap();

    let template_dir = runtime_dir.join("template");
    fs::create_dir_all(&template_dir).unwrap();
    fs::write(template_dir.join("CLAUDE.md"), "ok").unwrap();
    // Drop a symlink inside the template dir — should be rejected.
    std::os::unix::fs::symlink("/etc/passwd", template_dir.join("malicious")).unwrap();

    let err =
        vibepod::cli::run::prepare::assemble_staging(&config_dir, &runtime_dir, &template_dir)
            .unwrap_err();
    assert!(
        format!("{err}").contains("symlink"),
        "expected symlink rejection, got: {err}"
    );
}
