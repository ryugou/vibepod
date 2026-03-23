use tempfile::TempDir;
use vibepod::auth::{AuthManager, TokenData};

fn make_valid_token() -> TokenData {
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::days(365);
    TokenData {
        token: "sk-ant-oat01-test-token".to_string(),
        created_at: now.to_rfc3339(),
        expires_at: expires.to_rfc3339(),
    }
}

fn make_expired_token() -> TokenData {
    let now = chrono::Utc::now();
    let expired = now - chrono::Duration::hours(1);
    TokenData {
        token: "sk-ant-oat01-expired".to_string(),
        created_at: (now - chrono::Duration::days(366)).to_rfc3339(),
        expires_at: expired.to_rfc3339(),
    }
}

fn make_expiring_soon_token() -> TokenData {
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::days(3); // Within 7-day threshold
    TokenData {
        token: "sk-ant-oat01-expiring".to_string(),
        created_at: (now - chrono::Duration::days(362)).to_rfc3339(),
        expires_at: expires.to_rfc3339(),
    }
}

#[test]
fn test_save_and_load_token() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"));

    let token = make_valid_token();
    manager.save_token(&token).unwrap();
    let loaded = manager.load_token().unwrap().unwrap();
    assert_eq!(loaded.token, "sk-ant-oat01-test-token");
}

#[test]
fn test_load_token_not_exists() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"));
    assert!(manager.load_token().unwrap().is_none());
}

#[test]
fn test_token_not_expired() {
    let token = make_valid_token();
    assert!(!token.is_expired());
}

#[test]
fn test_token_expired() {
    let token = make_expired_token();
    assert!(token.is_expired());
}

#[test]
fn test_token_needs_renewal() {
    let token = make_expiring_soon_token();
    assert!(token.needs_renewal());
}

#[test]
fn test_token_does_not_need_renewal() {
    let token = make_valid_token();
    assert!(!token.needs_renewal());
}

#[test]
fn test_delete_token() {
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"));

    let token = make_valid_token();
    manager.save_token(&token).unwrap();
    manager.delete_token().unwrap();
    assert!(manager.load_token().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn test_file_permissions_600() {
    use std::os::unix::fs::MetadataExt;
    let dir = TempDir::new().unwrap();
    let manager = AuthManager::new(dir.path().join("config"));

    let token = make_valid_token();
    manager.save_token(&token).unwrap();

    let path = dir.path().join("config").join("auth").join("token.json");
    let mode = std::fs::metadata(&path).unwrap().mode();
    assert_eq!(mode & 0o777, 0o600);
}
