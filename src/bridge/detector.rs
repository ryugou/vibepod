use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum DetectorState {
    Buffering,
    WaitingResponse,
}

pub enum DetectorEvent {
    /// 無音検知。ANSI ストリップ済みの生テキストを含む（整形は formatter が行う）
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

        let content = self.buffer_for_slack();
        if content.trim().is_empty() {
            self.buffer.clear();
            self.last_output_at = None;
            return None;
        }

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

    /// ANSI エスケープをストリップし、末尾 40 行 / 2500 文字に切り詰めて返す。
    /// 整形は formatter に委譲。
    pub fn buffer_for_slack(&self) -> String {
        let stripped = strip_ansi_escapes::strip(&self.buffer);
        let text = String::from_utf8_lossy(&stripped).to_string();
        truncate_tail(&text, 40, 2500)
    }
}

/// 末尾 max_lines 行 / max_chars 文字に切り詰める。超過時は先頭に省略表示を付ける。
fn truncate_tail(text: &str, max_lines: usize, max_chars: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();

    // 行数制限
    let tail = if lines.len() > max_lines {
        &lines[lines.len() - max_lines..]
    } else {
        &lines
    };
    let mut result = tail.join("\n");

    // 文字数制限
    let char_count = result.chars().count();
    if char_count > max_chars {
        let skip = char_count - max_chars;
        let offset = result.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
        result = result[offset..].to_string();
    }

    if lines.len() > max_lines || result.chars().count() < text.chars().count() {
        format!("...\n{}", result)
    } else {
        result
    }
}
