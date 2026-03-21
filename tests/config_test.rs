use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_save_and_load_global_config() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().to_path_buf();

    let config = vibepod::config::GlobalConfig {
        default_agent: "claude".to_string(),
        image: "vibepod-claude:latest".to_string(),
        claude_version: "latest".to_string(),
    };

    vibepod::config::save_global_config(&config, &config_dir).unwrap();
    let loaded = vibepod::config::load_global_config(&config_dir).unwrap();

    assert_eq!(loaded.default_agent, "claude");
    assert_eq!(loaded.image, "vibepod-claude:latest");
    assert_eq!(loaded.claude_version, "latest");
}

#[test]
fn test_load_global_config_not_found() {
    let tmp = TempDir::new().unwrap();
    let result = vibepod::config::load_global_config(&tmp.path().to_path_buf());
    assert!(result.is_err());
}

#[test]
fn test_register_and_check_project() {
    let mut config = vibepod::config::ProjectsConfig::default();
    assert!(!vibepod::config::is_project_registered(&config, "/path/to/project"));

    vibepod::config::register_project(&mut config, vibepod::config::ProjectEntry {
        name: "my-project".to_string(),
        path: "/path/to/project".to_string(),
        remote: Some("github.com/user/repo".to_string()),
        registered_at: "2026-03-22T10:00:00Z".to_string(),
    });

    assert!(vibepod::config::is_project_registered(&config, "/path/to/project"));
    assert!(!vibepod::config::is_project_registered(&config, "/other/path"));
}
