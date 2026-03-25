use std::time::{Duration, Instant};

const MAX_LINES: usize = 40;
const MAX_CHARS: usize = 2500;

#[derive(Debug, Clone, PartialEq)]
pub enum DetectorState {
    Buffering,
    Idle,
    WaitingResponse,
}

pub enum DetectorEvent {
    Notify(String),
    ResponseReceived,
}

pub struct IdleDetector {
    state: DetectorState,
    buffer: Vec<u8>,
    delay: Duration,
    last_output_at: Option<Instant>,
}

impl IdleDetector {
    pub fn new(delay: Duration) -> Self {
        Self {
            state: DetectorState::Buffering,
            buffer: Vec::new(),
            delay,
            last_output_at: None,
        }
    }

    pub fn state(&self) -> DetectorState {
        self.state.clone()
    }

    pub fn on_output(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        self.last_output_at = Some(Instant::now());
    }

    pub fn on_terminal_input(&mut self) {
        if self.state == DetectorState::Buffering {
            self.buffer.clear();
            self.last_output_at = Some(Instant::now());
        }
    }

    pub fn check_idle(&mut self) -> Option<DetectorEvent> {
        if self.state != DetectorState::Buffering {
            return None;
        }

        let last_output = self.last_output_at?;
        if last_output.elapsed() < self.delay {
            return None;
        }

        if self.buffer.is_empty() {
            return None;
        }

        self.state = DetectorState::Idle;
        let content = self.buffer_for_slack();
        self.state = DetectorState::WaitingResponse;
        Some(DetectorEvent::Notify(content))
    }

    pub fn on_response(&mut self) {
        if self.state == DetectorState::WaitingResponse {
            self.state = DetectorState::Buffering;
            self.buffer.clear();
            self.last_output_at = None;
        }
    }

    pub fn on_output_resumed(&mut self) {
        if self.state == DetectorState::WaitingResponse {
            self.state = DetectorState::Buffering;
            self.buffer.clear();
            self.last_output_at = None;
        }
    }

    pub fn buffer_content(&self) -> String {
        String::from_utf8_lossy(&self.buffer).to_string()
    }

    pub fn buffer_for_slack(&self) -> String {
        let stripped = strip_ansi_escapes::strip(&self.buffer);
        let text = String::from_utf8_lossy(&stripped);

        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();

        // Keep the last MAX_LINES lines (tail)
        let (selected_lines, truncated_count) = if total_lines > MAX_LINES {
            (&lines[total_lines - MAX_LINES..], total_lines - MAX_LINES)
        } else {
            (lines.as_slice(), 0)
        };

        let mut result = selected_lines.join("\n");

        // Truncate by character count
        if result.len() > MAX_CHARS {
            // Find a safe truncation point
            let truncated = &result[result.len() - MAX_CHARS..];
            result = format!("...{}", truncated);
        }

        if truncated_count > 0 {
            result = format!("... ({} lines truncated)\n{}", truncated_count, result);
        }

        result
    }
}
