use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn sessions_path(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }

    pub fn load(&self) -> Result<SessionsData> {
        let path = self.sessions_path();
        if !path.exists() {
            return Ok(SessionsData::default());
        }
        let json = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let data: SessionsData =
            serde_json::from_str(&json).context("Session history file is corrupted")?;
        Ok(data)
    }

    pub fn save(&self, data: &SessionsData) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::create_dir_all(self.dir.join("reports"))?;
        let json = serde_json::to_string_pretty(data)?;
        fs::write(self.sessions_path(), json)?;
        Ok(())
    }

    pub fn add(&self, session: Session) -> Result<()> {
        let mut data = self.load()?;
        data.sessions.push(session);
        // Remove oldest entries if over limit
        if data.sessions.len() > MAX_SESSIONS {
            let remove_count = data.sessions.len() - MAX_SESSIONS;
            data.sessions.drain(..remove_count);
        }
        self.save(&data)
    }

    pub fn mark_restored(&self, session_id: &str) -> Result<()> {
        let mut data = self.load()?;
        if let Some(session) = data.sessions.iter_mut().find(|s| s.id == session_id) {
            session.restored = true;
        }
        self.save(&data)
    }

    /// Mark the specified session and all subsequent sessions as restored
    pub fn mark_restored_since(&self, session_id: &str) -> Result<()> {
        let mut data = self.load()?;
        let mut found = false;
        for session in data.sessions.iter_mut() {
            if session.id == session_id {
                found = true;
            }
            if found {
                session.restored = true;
            }
        }
        self.save(&data)
    }

    pub fn restorable_sessions(&self) -> Result<Vec<Session>> {
        let data = self.load()?;
        Ok(data.sessions.into_iter().filter(|s| !s.restored).collect())
    }

    pub fn reports_dir(&self) -> PathBuf {
        self.dir.join("reports")
    }
}

pub fn generate_session_id() -> String {
    let now = chrono::Local::now();
    let suffix: String = (0..4)
        .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
        .collect();
    format!("{}-{}", now.format("%Y%m%d-%H%M%S"), suffix)
}
