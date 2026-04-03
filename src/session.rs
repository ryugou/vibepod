use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub started_at: String,
    pub head_before: String,
    pub branch: String,
    pub prompt: String,
    pub claude_session_path: Option<String>,
    pub restored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionsData {
    pub sessions: Vec<Session>,
}

const MAX_SESSIONS: usize = 100;

/// セッション履歴管理
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    /// Returns the directory for a specific session: `.vibepod/sessions/{id}/`
    pub fn session_dir(&self, id: &str) -> PathBuf {
        self.sessions_dir().join(id)
    }

    pub fn load(&self) -> Result<SessionsData> {
        let sessions_dir = self.sessions_dir();

        // Collect sessions from per-session directories, keyed by id for dedup
        let mut sessions_map: HashMap<String, Session> = HashMap::new();

        if sessions_dir.exists() {
            let entries = fs::read_dir(&sessions_dir)
                .with_context(|| format!("Failed to read {}", sessions_dir.display()))?;
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    let metadata_path = path.join("metadata.json");
                    if metadata_path.exists() {
                        let json = fs::read_to_string(&metadata_path).with_context(|| {
                            format!("Failed to read {}", metadata_path.display())
                        })?;
                        let session: Session = serde_json::from_str(&json).with_context(|| {
                            format!("Session metadata is corrupted: {}", metadata_path.display())
                        })?;
                        sessions_map.insert(session.id.clone(), session);
                    }
                }
            }
        }

        // Always check for legacy sessions.json and migrate any sessions not yet moved.
        // If the file is corrupt, propagate the error rather than silently deleting it.
        let legacy_path = self.dir.join("sessions.json");
        if legacy_path.exists() {
            let json = fs::read_to_string(&legacy_path)
                .with_context(|| format!("Failed to read {}", legacy_path.display()))?;
            let data: SessionsData =
                serde_json::from_str(&json).context("Session history file is corrupted")?;
            for session in data.sessions {
                if !sessions_map.contains_key(&session.id) {
                    // Migrate to per-session directory structure; propagate I/O errors so
                    // sessions.json is only removed after every session is written successfully.
                    let dir = self.session_dir(&session.id);
                    fs::create_dir_all(&dir)
                        .with_context(|| format!("Failed to create {}", dir.display()))?;
                    let path = dir.join("metadata.json");
                    let json = serde_json::to_string_pretty(&session)?;
                    fs::write(&path, json)
                        .with_context(|| format!("Failed to write {}", path.display()))?;
                    sessions_map.insert(session.id.clone(), session);
                }
            }
            // Only delete after every session has been successfully migrated
            fs::remove_file(&legacy_path).ok();
        }

        if sessions_map.is_empty() {
            return Ok(SessionsData::default());
        }

        let mut sessions: Vec<Session> = sessions_map.into_values().collect();
        // Sort by ID (lexicographic). IDs are YYYYMMDD-HHMMSS-xxxx, generated at insertion
        // time, so this approximates the original append order without relying on wall clock
        // comparisons that can be disrupted by clock skew or DST changes.
        sessions.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(SessionsData { sessions })
    }

    pub fn save(&self, data: &SessionsData) -> Result<()> {
        let sessions_dir = self.sessions_dir();
        fs::create_dir_all(&sessions_dir)?;

        // Remove directories for sessions not present in the input (full-overwrite semantics)
        let ids_to_keep: std::collections::HashSet<&str> =
            data.sessions.iter().map(|s| s.id.as_str()).collect();
        if let Ok(entries) = fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let dir_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default();
                    if !ids_to_keep.contains(dir_name) {
                        fs::remove_dir_all(&path).ok();
                    }
                }
            }
        }

        for session in &data.sessions {
            let dir = self.session_dir(&session.id);
            fs::create_dir_all(&dir)?;
            let path = dir.join("metadata.json");
            let json = serde_json::to_string_pretty(session)?;
            fs::write(path, json)?;
        }
        Ok(())
    }

    pub fn add(&self, session: Session) -> Result<()> {
        // Ensure any legacy sessions.json is migrated before we add the new session,
        // so the migration check in load() sees a consistent state.
        let _ = self.load()?;

        let session_dir = self.session_dir(&session.id);
        fs::create_dir_all(&session_dir)?;
        let path = session_dir.join("metadata.json");
        let json = serde_json::to_string_pretty(&session)?;
        fs::write(path, json)?;

        // Enforce MAX_SESSIONS by removing the oldest session directories
        let data = self.load()?;
        if data.sessions.len() > MAX_SESSIONS {
            let remove_count = data.sessions.len() - MAX_SESSIONS;
            for old_session in &data.sessions[..remove_count] {
                let dir = self.session_dir(&old_session.id);
                fs::remove_dir_all(&dir).ok();
            }
        }

        Ok(())
    }

    pub fn mark_restored(&self, session_id: &str) -> Result<()> {
        let path = self.session_dir(session_id).join("metadata.json");
        if !path.exists() {
            return Ok(());
        }
        let json = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let mut session: Session =
            serde_json::from_str(&json).context("Session metadata is corrupted")?;
        session.restored = true;
        let json = serde_json::to_string_pretty(&session)?;
        fs::write(&path, json)?;
        Ok(())
    }

    /// Mark the specified session and all subsequent sessions as restored
    pub fn mark_restored_since(&self, session_id: &str) -> Result<()> {
        let data = self.load()?;
        let mut found = false;
        for session in &data.sessions {
            if session.id == session_id {
                found = true;
            }
            if found {
                self.mark_restored(&session.id)?;
            }
        }
        Ok(())
    }

    pub fn restorable_sessions(&self) -> Result<Vec<Session>> {
        let data = self.load()?;
        Ok(data.sessions.into_iter().filter(|s| !s.restored).collect())
    }

    pub fn last_session_id(&self) -> Option<String> {
        let data = self.load().ok()?;
        data.sessions.last().map(|s| s.id.clone())
    }
}

pub fn generate_session_id() -> String {
    let now = chrono::Local::now();
    let suffix: String = (0..4)
        .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
        .collect();
    format!("{}-{}", now.format("%Y%m%d-%H%M%S"), suffix)
}
