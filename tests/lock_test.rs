use std::fs;

#[test]
fn test_lock_acquire_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join(".vibepod/prompt.lock");
    let vibepod_dir = dir.path().join(".vibepod");
    fs::create_dir_all(&vibepod_dir).unwrap();

    let lock = vibepod::cli::run::lock::PromptLock::acquire(
        vibepod_dir.clone(),
        "test prompt".to_string(),
    )
    .unwrap();

    assert!(lock_path.exists());
    let content: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    assert_eq!(content["prompt"], "test prompt");
    assert!(content["pid"].as_u64().unwrap() > 0);
    assert!(content["started_at"].as_str().is_some());
    assert!(content["last_event_at"].as_str().is_some());

    drop(lock);
    assert!(!lock_path.exists());
}

#[test]
fn test_lock_check_no_lock() {
    let dir = tempfile::tempdir().unwrap();
    let vibepod_dir = dir.path().join(".vibepod");
    fs::create_dir_all(&vibepod_dir).unwrap();

    let result = vibepod::cli::run::lock::PromptLock::check(&vibepod_dir);
    assert!(result.is_none());
}

#[test]
fn test_lock_check_stale_lock_auto_removed() {
    let dir = tempfile::tempdir().unwrap();
    let vibepod_dir = dir.path().join(".vibepod");
    fs::create_dir_all(&vibepod_dir).unwrap();
    let lock_path = vibepod_dir.join("prompt.lock");

    let content = serde_json::json!({
        "pid": 999999999,
        "started_at": "2026-04-05T10:00:00+09:00",
        "prompt": "stale",
        "last_event_at": "2026-04-05T10:00:00+09:00"
    });
    fs::write(&lock_path, serde_json::to_string(&content).unwrap()).unwrap();

    let result = vibepod::cli::run::lock::PromptLock::check(&vibepod_dir);
    assert!(result.is_none());
    assert!(!lock_path.exists());
}

#[test]
fn test_lock_check_active_lock_returns_pid() {
    let dir = tempfile::tempdir().unwrap();
    let vibepod_dir = dir.path().join(".vibepod");
    fs::create_dir_all(&vibepod_dir).unwrap();
    let lock_path = vibepod_dir.join("prompt.lock");

    let my_pid = std::process::id();
    let content = serde_json::json!({
        "pid": my_pid,
        "started_at": "2026-04-05T10:00:00+09:00",
        "prompt": "active",
        "last_event_at": "2026-04-05T10:00:00+09:00"
    });
    fs::write(&lock_path, serde_json::to_string(&content).unwrap()).unwrap();

    let result = vibepod::cli::run::lock::PromptLock::check(&vibepod_dir);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), my_pid);

    fs::remove_file(&lock_path).ok();
}

#[test]
fn test_lock_update_last_event() {
    let dir = tempfile::tempdir().unwrap();
    let vibepod_dir = dir.path().join(".vibepod");
    fs::create_dir_all(&vibepod_dir).unwrap();

    let lock =
        vibepod::cli::run::lock::PromptLock::acquire(vibepod_dir.clone(), "test".to_string())
            .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(10));
    lock.update_last_event().unwrap();

    let content: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(vibepod_dir.join("prompt.lock")).unwrap())
            .unwrap();
    assert!(content["last_event_at"].as_str().is_some());
}
