use tempfile::TempDir;
use vibepod::session::{Session, SessionStore};

#[test]
fn test_add_and_load_session() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    let session = Session {
        id: "20260323-120000-a3f2".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: false,
    };

    store.add(session.clone()).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].id, "20260323-120000-a3f2");
    assert_eq!(loaded.sessions[0].head_before, "abc1234");
}

#[test]
fn test_session_limit_100() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    for i in 0..105 {
        let session = Session {
            id: format!("session-{:04}", i),
            started_at: format!("2026-03-23T{:02}:00:00+09:00", i % 24),
            head_before: format!("{:040x}", i),
            branch: "main".to_string(),
            prompt: "interactive".to_string(),
            claude_session_path: None,
            restored: false,
        };
        store.add(session).unwrap();
    }

    let loaded = store.load().unwrap();
    assert_eq!(loaded.sessions.len(), 100);
    // 最新 100 件が残る（0-4 が削除される）
    assert_eq!(loaded.sessions[0].id, "session-0005");
}

#[test]
fn test_mark_restored() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    let session = Session {
        id: "test-session".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: false,
    };
    store.add(session).unwrap();

    store.mark_restored("test-session").unwrap();
    let loaded = store.load().unwrap();
    assert!(loaded.sessions[0].restored);
}

#[test]
fn test_mark_restored_since() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    for id in ["s1", "s2", "s3"] {
        let session = Session {
            id: id.to_string(),
            started_at: "2026-03-23T12:00:00+09:00".to_string(),
            head_before: "abc".to_string(),
            branch: "main".to_string(),
            prompt: "interactive".to_string(),
            claude_session_path: None,
            restored: false,
        };
        store.add(session).unwrap();
    }

    store.mark_restored_since("s2").unwrap();
    let loaded = store.load().unwrap();
    assert!(!loaded.sessions[0].restored); // s1: unchanged
    assert!(loaded.sessions[1].restored); // s2: restored
    assert!(loaded.sessions[2].restored); // s3: restored
}

#[test]
fn test_restorable_sessions() {
    let dir = TempDir::new().unwrap();
    let store = SessionStore::new(dir.path().join(".vibepod"));

    let s1 = Session {
        id: "s1".to_string(),
        started_at: "2026-03-23T10:00:00+09:00".to_string(),
        head_before: "aaa".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: true,
    };
    let s2 = Session {
        id: "s2".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "bbb".to_string(),
        branch: "main".to_string(),
        prompt: "--resume".to_string(),
        claude_session_path: None,
        restored: false,
    };
    store.add(s1).unwrap();
    store.add(s2).unwrap();

    let restorable = store.restorable_sessions().unwrap();
    assert_eq!(restorable.len(), 1);
    assert_eq!(restorable[0].id, "s2");
}

#[test]
fn test_generate_session_id_unique() {
    let id1 = vibepod::session::generate_session_id();
    let id2 = vibepod::session::generate_session_id();
    assert_ne!(id1, id2);
}
