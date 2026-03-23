use tempfile::TempDir;
use vibepod::auth::{AuthManager, Credentials};

fn make_valid_credentials() -> Credentials {
    Credentials {
        claude_ai_oauth: serde_json::json!({
            "accessToken": "test-token",
            "refreshToken": "test-refresh",
            "expiresAt": (chrono::Utc::now().timestamp_millis() + 3600000) // 1 hour from now
        }),
    }
}

fn make_expired_credentials() -> Credentials {
    Credentials {
        claude_ai_oauth: serde_json::json!({
            "accessToken": "test-token",
            "refreshToken": "test-refresh",
            "expiresAt": (chrono::Utc::now().timestamp_millis() - 3600000) // 1 hour ago
        }),
    }
}

#[test]
fn test_save_and_load_shared_credentials() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();
    let loaded = manager.load_shared().unwrap().unwrap();
    assert_eq!(loaded.claude_ai_oauth["accessToken"], "test-token");
}

#[test]
fn test_load_shared_not_exists() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));
    assert!(manager.load_shared().unwrap().is_none());
}

#[test]
fn test_credentials_not_expired() {
    let creds = make_valid_credentials();
    assert!(!creds.is_expired());
}

#[test]
fn test_credentials_expired() {
    let creds = make_expired_credentials();
    assert!(creds.is_expired());
}

#[test]
fn test_save_and_load_isolated_credentials() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_isolated("my-container", &creds).unwrap();
    let loaded = manager.load_isolated("my-container").unwrap().unwrap();
    assert_eq!(loaded.claude_ai_oauth["accessToken"], "test-token");
}

#[test]
fn test_lock_acquire_and_release() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    assert!(manager.try_acquire_lock("container-1").unwrap());
    assert!(!manager.try_acquire_lock("container-2").unwrap());
    manager.release_lock().unwrap();
    assert!(manager.try_acquire_lock("container-2").unwrap());
}

#[test]
fn test_lock_info() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    manager.try_acquire_lock("my-container").unwrap();
    let info = manager.lock_info().unwrap();
    assert_eq!(info, Some("my-container".to_string()));
}

#[test]
fn test_copy_to_temp_and_writeback() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();

    let temp_path = manager.copy_shared_to_temp().unwrap();
    assert!(temp_path.exists());

    // Simulate token refresh by modifying temp file
    let mut updated = creds.clone();
    updated.claude_ai_oauth = serde_json::json!({
        "accessToken": "refreshed-token",
        "refreshToken": "test-refresh",
        "expiresAt": (chrono::Utc::now().timestamp_millis() + 7200000)
    });
    let json = serde_json::to_string_pretty(&updated).unwrap();
    std::fs::write(&temp_path, &json).unwrap();

    manager.writeback_shared(&temp_path).unwrap();

    let reloaded = manager.load_shared().unwrap().unwrap();
    assert_eq!(reloaded.claude_ai_oauth["accessToken"], "refreshed-token");
}

#[test]
fn test_delete_shared() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();
    manager.try_acquire_lock("test").unwrap();

    manager.delete_shared().unwrap();
    assert!(manager.load_shared().unwrap().is_none());
    assert!(manager.lock_info().unwrap().is_none());
}

#[test]
fn test_delete_all_isolated() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_isolated("c1", &creds).unwrap();
    manager.save_isolated("c2", &creds).unwrap();

    manager.delete_all_isolated().unwrap();
    assert!(manager.load_isolated("c1").unwrap().is_none());
    assert!(manager.load_isolated("c2").unwrap().is_none());
}

#[test]
fn test_url_detection() {
    assert!(vibepod::auth::detect_oauth_url(
        "Visit https://platform.claude.com/oauth/authorize?foo=bar to log in"
    )
    .is_some());
    assert!(vibepod::auth::detect_oauth_url("No URL here").is_none());
    assert!(
        vibepod::auth::detect_oauth_url("Go to https://claude.ai/oauth/authorize?x=1").is_some()
    );
}

#[cfg(unix)]
#[test]
fn test_file_permissions_600() {
    use std::os::unix::fs::MetadataExt;
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"), dir.path().join("project"));

    let creds = make_valid_credentials();
    manager.save_shared(&creds).unwrap();

    let path = dir
        .path()
        .join("config")
        .join("auth")
        .join("credentials.json");
    let mode = std::fs::metadata(&path).unwrap().mode();
    assert_eq!(mode & 0o777, 0o600);
}
