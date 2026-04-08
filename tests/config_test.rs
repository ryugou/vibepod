use std::fs;
use tempfile::TempDir;

#[test]
fn test_save_and_load_global_config() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().to_path_buf();

    let config = vibepod::config::GlobalConfig {
        default_agent: "claude".to_string(),
        image: "vibepod-claude:latest".to_string(),
    };

    vibepod::config::save_global_config(&config, &config_dir).unwrap();
    let loaded = vibepod::config::load_global_config(&config_dir).unwrap();

    assert_eq!(loaded.default_agent, "claude");
    assert_eq!(loaded.image, "vibepod-claude:latest");
}

#[test]
fn test_load_global_config_not_found() {
    let tmp = TempDir::new().unwrap();
    let result = vibepod::config::load_global_config(tmp.path());
    assert!(result.is_err());
}

#[test]
fn test_register_and_check_project() {
    let mut config = vibepod::config::ProjectsConfig::default();
    assert!(!vibepod::config::is_project_registered(
        &config,
        "/path/to/project"
    ));

    vibepod::config::register_project(
        &mut config,
        vibepod::config::ProjectEntry {
            name: "my-project".to_string(),
            path: "/path/to/project".to_string(),
            remote: Some("github.com/user/repo".to_string()),
            registered_at: "2026-03-22T10:00:00Z".to_string(),
        },
    );

    assert!(vibepod::config::is_project_registered(
        &config,
        "/path/to/project"
    ));
    assert!(!vibepod::config::is_project_registered(
        &config,
        "/other/path"
    ));
}

#[test]
fn test_save_and_load_projects() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().to_path_buf();

    let mut config = vibepod::config::ProjectsConfig::default();
    vibepod::config::register_project(
        &mut config,
        vibepod::config::ProjectEntry {
            name: "test-project".to_string(),
            path: "/path/to/test".to_string(),
            remote: Some("github.com/user/test".to_string()),
            registered_at: "2026-03-22T10:00:00Z".to_string(),
        },
    );

    vibepod::config::save_projects(&config, &config_dir).unwrap();
    let loaded = vibepod::config::load_projects(&config_dir).unwrap();

    assert_eq!(loaded.projects.len(), 1);
    assert_eq!(loaded.projects[0].name, "test-project");
    assert_eq!(loaded.projects[0].path, "/path/to/test");
    assert_eq!(
        loaded.projects[0].remote,
        Some("github.com/user/test".to_string())
    );
}

// --- VibepodConfig tests ---

#[test]
fn test_load_vibepod_config_project_only() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    fs::create_dir_all(project_dir.join(".vibepod")).unwrap();
    fs::write(
        project_dir.join(".vibepod/config.toml"),
        "[run]\nlang = \"rust\"\n",
    )
    .unwrap();
    let global_dir = tmp.path().join("global");
    fs::create_dir_all(&global_dir).unwrap();

    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    assert_eq!(config.lang(), Some("rust".to_string()));
}

#[test]
fn test_load_vibepod_config_global_only() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    fs::create_dir_all(&project_dir).unwrap();
    let global_dir = tmp.path().join("global");
    fs::create_dir_all(&global_dir).unwrap();
    fs::write(global_dir.join("config.toml"), "[run]\nlang = \"go\"\n").unwrap();

    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    assert_eq!(config.lang(), Some("go".to_string()));
}

#[test]
fn test_load_vibepod_config_merge_priority() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    fs::create_dir_all(project_dir.join(".vibepod")).unwrap();
    fs::write(
        project_dir.join(".vibepod/config.toml"),
        "[run]\nlang = \"node\"\n",
    )
    .unwrap();
    let global_dir = tmp.path().join("global");
    fs::create_dir_all(&global_dir).unwrap();
    fs::write(global_dir.join("config.toml"), "[run]\nlang = \"python\"\n").unwrap();

    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    // project overrides global for lang
    assert_eq!(config.lang(), Some("node".to_string()));
}

#[test]
fn test_load_vibepod_config_none() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    fs::create_dir_all(&project_dir).unwrap();
    let global_dir = tmp.path().join("global");
    fs::create_dir_all(&global_dir).unwrap();

    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    assert_eq!(config.lang(), None);
}

#[test]
fn test_prompt_idle_timeout_default() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let config = vibepod::config::VibepodConfig::load(dir.path(), config_dir.path()).unwrap();
    assert_eq!(config.prompt_idle_timeout(), 300);
}

#[test]
fn test_prompt_idle_timeout_custom() {
    let dir = tempfile::tempdir().unwrap();
    let vibepod_dir = dir.path().join(".vibepod");
    std::fs::create_dir_all(&vibepod_dir).unwrap();
    std::fs::write(
        vibepod_dir.join("config.toml"),
        "[run]\nprompt_idle_timeout = 600\n",
    )
    .unwrap();

    let config_dir = tempfile::tempdir().unwrap();
    let config = vibepod::config::VibepodConfig::load(dir.path(), config_dir.path()).unwrap();
    assert_eq!(config.prompt_idle_timeout(), 600);
}

#[test]
fn test_prompt_idle_timeout_zero_disables() {
    let dir = tempfile::tempdir().unwrap();
    let vibepod_dir = dir.path().join(".vibepod");
    std::fs::create_dir_all(&vibepod_dir).unwrap();
    std::fs::write(
        vibepod_dir.join("config.toml"),
        "[run]\nprompt_idle_timeout = 0\n",
    )
    .unwrap();

    let config_dir = tempfile::tempdir().unwrap();
    let config = vibepod::config::VibepodConfig::load(dir.path(), config_dir.path()).unwrap();
    assert_eq!(config.prompt_idle_timeout(), 0);
}

// --- default_prompt_template (Phase 2 では field のみ、挙動は Phase 4) ---

#[test]
fn test_default_prompt_template_not_set() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let config = vibepod::config::VibepodConfig::load(dir.path(), config_dir.path()).unwrap();
    assert_eq!(config.default_prompt_template(), None);
}

#[test]
fn test_default_prompt_template_from_project() {
    let dir = tempfile::tempdir().unwrap();
    let vibepod_dir = dir.path().join(".vibepod");
    std::fs::create_dir_all(&vibepod_dir).unwrap();
    std::fs::write(
        vibepod_dir.join("config.toml"),
        "[run]\ndefault_prompt_template = \"rust-code\"\n",
    )
    .unwrap();

    let config_dir = tempfile::tempdir().unwrap();
    let config = vibepod::config::VibepodConfig::load(dir.path(), config_dir.path()).unwrap();
    assert_eq!(
        config.default_prompt_template(),
        Some("rust-code".to_string())
    );
}

#[test]
fn test_default_prompt_template_from_global() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = dir.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let global_dir = dir.path().join("global");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(
        global_dir.join("config.toml"),
        "[run]\ndefault_prompt_template = \"review\"\n",
    )
    .unwrap();

    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    assert_eq!(config.default_prompt_template(), Some("review".to_string()));
}

#[test]
fn test_default_prompt_template_project_overrides_global() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = dir.path().join("project");
    std::fs::create_dir_all(project_dir.join(".vibepod")).unwrap();
    std::fs::write(
        project_dir.join(".vibepod/config.toml"),
        "[run]\ndefault_prompt_template = \"custom\"\n",
    )
    .unwrap();
    let global_dir = dir.path().join("global");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(
        global_dir.join("config.toml"),
        "[run]\ndefault_prompt_template = \"rust-code\"\n",
    )
    .unwrap();

    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    assert_eq!(config.default_prompt_template(), Some("custom".to_string()));
}

// --- set_default_prompt_template write API (Phase 3) ---

#[test]
fn test_set_default_prompt_template_creates_new_config() {
    let global_dir = tempfile::tempdir().unwrap();
    vibepod::config::set_default_prompt_template(global_dir.path(), Some("rust-code")).unwrap();

    let config = vibepod::config::VibepodConfig::load(
        &std::path::PathBuf::from("/nonexistent"),
        global_dir.path(),
    )
    .unwrap();
    assert_eq!(
        config.default_prompt_template(),
        Some("rust-code".to_string())
    );
}

#[test]
fn test_set_default_prompt_template_updates_existing_config() {
    let global_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        global_dir.path().join("config.toml"),
        "[run]\nlang = \"rust\"\n",
    )
    .unwrap();

    vibepod::config::set_default_prompt_template(global_dir.path(), Some("review")).unwrap();

    let config = vibepod::config::VibepodConfig::load(
        &std::path::PathBuf::from("/nonexistent"),
        global_dir.path(),
    )
    .unwrap();
    assert_eq!(config.lang(), Some("rust".to_string()));
    assert_eq!(config.default_prompt_template(), Some("review".to_string()));
}

#[test]
fn test_set_default_prompt_template_removes_key_when_none() {
    let global_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        global_dir.path().join("config.toml"),
        "[run]\nlang = \"rust\"\ndefault_prompt_template = \"review\"\n",
    )
    .unwrap();

    vibepod::config::set_default_prompt_template(global_dir.path(), None).unwrap();

    let config = vibepod::config::VibepodConfig::load(
        &std::path::PathBuf::from("/nonexistent"),
        global_dir.path(),
    )
    .unwrap();
    assert_eq!(config.lang(), Some("rust".to_string()));
    assert_eq!(config.default_prompt_template(), None);
}

#[test]
fn test_set_default_prompt_template_preserves_unknown_sections() {
    let global_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        global_dir.path().join("config.toml"),
        "[global]\ndefault_agent = \"claude\"\nimage = \"vibepod:test\"\n\n[run]\nlang = \"rust\"\n",
    )
    .unwrap();

    vibepod::config::set_default_prompt_template(global_dir.path(), Some("rust-code")).unwrap();

    let content = std::fs::read_to_string(global_dir.path().join("config.toml")).unwrap();
    assert!(
        content.contains("default_agent"),
        "global section should be preserved, got: {}",
        content
    );
    assert!(content.contains("default_prompt_template"));
    assert!(content.contains("rust-code"));
}
