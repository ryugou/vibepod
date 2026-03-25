use anyhow::Result;
use chrono::Local;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct NotifiedEvent<'a> {
    ts: String,
    event: &'static str,
    last_lines: &'a str,
}

#[derive(Serialize)]
struct RespondedEvent<'a> {
    ts: String,
    event: &'static str,
    source: &'a str,
    stdin_sent: &'a str,
    response_time_seconds: u64,
}

pub struct BridgeLogger {
    path: PathBuf,
}

impl BridgeLogger {
    pub fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    pub fn log_notified(&mut self, last_lines: &str) -> Result<()> {
        let event = NotifiedEvent {
            ts: Local::now().to_rfc3339(),
            event: "notified",
            last_lines,
        };
        self.append(&serde_json::to_string(&event)?)
    }

    pub fn log_responded(
        &mut self,
        source: &str,
        stdin_sent: &str,
        response_time_seconds: u64,
    ) -> Result<()> {
        let event = RespondedEvent {
            ts: Local::now().to_rfc3339(),
            event: "responded",
            source,
            stdin_sent,
            response_time_seconds,
        };
        self.append(&serde_json::to_string(&event)?)
    }

    fn append(&self, line: &str) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }
}
