use std::time::{Duration, Instant};

const MAX_LINES: usize = 40;
const MAX_CHARS: usize = 2500;
/// 意味のあるテキスト（装飾文字・空白以外）の最小文字数
const MIN_MEANINGFUL_CHARS: usize = 5;

/// 行が意味のあるテキストを含むかを判定する。
/// 罫線・空行・短すぎる行を除外して、TUI の装飾ノイズを取り除く。
fn is_meaningful_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // 英数字・日本語等の「テキスト」文字の数をカウント
    let text_chars: usize = trimmed
        .chars()
        .filter(|c| {
            c.is_alphanumeric()
                || matches!(*c, '?' | '!' | '(' | ')' | ':' | ',' | '。' | '、' | '？' | '！')
        })
        .count();
    // テキスト文字が2文字未満の行は装飾とみなす
    text_chars >= 2
}

/// バッファの内容が通知に値するかを判定する。
/// 罫線文字・空白・記号だけの出力（UI バナー等）はノイズなのでスキップ。
fn has_meaningful_content(text: &str) -> bool {
    let meaningful: usize = text
        .chars()
        .filter(|c| {
            // 罫線・ボックス描画文字 (U+2500-U+257F), 装飾記号, 空白, 制御文字を除外
            !c.is_whitespace()
                && !matches!(*c,
                    '\u{2500}'..='\u{257F}' | // Box Drawing (includes ─ ━ │ ┃ ╭ ╮ ╰ ╯ ║ ═ etc.)
                    '\u{2580}'..='\u{259F}' | // Block Elements (includes ▐ ▛ ▜ ▝ ▘ etc.)
                    '\u{2800}'..='\u{28FF}' | // Braille Patterns
                    '◇' | '◆' | '◐' | '◑' | '·' |
                    '.' | '*' | '-' | '=' | '|' | '+' | '>' | '<' | '`'
                )
        })
        .count();
    meaningful >= MIN_MEANINGFUL_CHARS
}

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

        let content = self.buffer_for_slack();

        // 意味のあるテキストがなければ通知しない（UI 装飾のみの場合をスキップ）
        if !has_meaningful_content(&content) {
            // バッファをクリアして再検知に備える
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

    pub fn buffer_for_slack(&self) -> String {
        let stripped = strip_ansi_escapes::strip(&self.buffer);
        let text = String::from_utf8_lossy(&stripped);

        // TUI 出力から装飾行を除去し、意味のあるテキスト行だけを残す
        let meaningful_lines: Vec<&str> = text
            .lines()
            .filter(|line| is_meaningful_line(line))
            .collect();

        let total = meaningful_lines.len();

        // 末尾 MAX_LINES 行を取得
        let (selected, truncated_count) = if total > MAX_LINES {
            (&meaningful_lines[total - MAX_LINES..], total - MAX_LINES)
        } else {
            (meaningful_lines.as_slice(), 0)
        };

        let mut result = selected.join("\n");

        // 文字数上限（char boundary safe）
        let char_count: usize = result.chars().count();
        if char_count > MAX_CHARS {
            let skip = char_count - MAX_CHARS;
            let byte_offset = result.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
            result = format!("...{}", &result[byte_offset..]);
        }

        if truncated_count > 0 {
            result = format!("... ({} lines truncated)\n{}", truncated_count, result);
        }

        result
    }
}
