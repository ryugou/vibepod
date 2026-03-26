// LLM-based text formatter for cleaning TUI output before Slack notification

use anyhow::{bail, Context, Result};
use serde_json::json;

#[derive(Debug, Clone, PartialEq)]
pub enum LlmProvider {
    Anthropic,
    Gemini,
    OpenAi,
    None,
}

impl LlmProvider {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "gemini" | "google" => Ok(Self::Gemini),
            "openai" | "gpt" => Ok(Self::OpenAi),
            "none" | "local" => Ok(Self::None),
            _ => bail!("Unknown LLM provider: '{}'. Use: anthropic, gemini, openai, or none", s),
        }
    }

    pub fn env_key_name(&self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Gemini => Some("GEMINI_API_KEY"),
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::None => None,
        }
    }
}

const SYSTEM_PROMPT: &str = "\
You are a text extraction tool. Given raw terminal output from a TUI application (Claude Code), \
extract ONLY the meaningful text content. \
Remove all UI decorations: box-drawing characters, spinner text, prompt symbols (❯ ● ✶), \
status indicators, shortcut hints, and any other TUI artifacts. \
Preserve the actual content structure: questions, options (a/b/c), messages, code snippets. \
Return ONLY the cleaned text, no explanations. Keep it concise. \
If the input is just UI noise with no meaningful content, return exactly: [no content]";

pub struct Formatter {
    provider: LlmProvider,
    api_key: String,
    http: reqwest::Client,
}

impl Formatter {
    pub fn new(provider: LlmProvider, api_key: String) -> Self {
        Self {
            provider,
            api_key,
            http: reqwest::Client::new(),
        }
    }

    /// TUI 出力を LLM で整形する。失敗時は ANSI ストリップ済みの生テキストを返す。
    pub async fn format(&self, raw_text: &str) -> String {
        if self.provider == LlmProvider::None {
            return local_format(raw_text);
        }
        match self.call_llm(raw_text).await {
            Ok(cleaned) => {
                if cleaned.trim() == "[no content]" {
                    String::new()
                } else {
                    cleaned
                }
            }
            Err(e) => {
                eprintln!("Warning: LLM formatting failed, using raw text: {}", e);
                local_format(raw_text)
            }
        }
    }

    async fn call_llm(&self, text: &str) -> Result<String> {
        // 入力をトークン節約のため切り詰め（末尾 3000 文字）
        let input = if text.chars().count() > 3000 {
            let skip = text.chars().count() - 3000;
            let offset = text.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
            &text[offset..]
        } else {
            text
        };

        match self.provider {
            LlmProvider::Anthropic => self.call_anthropic(input).await,
            LlmProvider::Gemini => self.call_gemini(input).await,
            LlmProvider::OpenAi => self.call_openai(input).await,
            LlmProvider::None => unreachable!("None provider handled in format()"),
        }
    }

    async fn call_anthropic(&self, text: &str) -> Result<String> {
        let body = json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 1024,
            "system": SYSTEM_PROMPT,
            "messages": [{"role": "user", "content": text}]
        });

        let http_resp = self.http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("Anthropic API request failed")?;

        let status = http_resp.status();
        let resp: serde_json::Value = http_resp.json().await?;

        if !status.is_success() {
            let msg = resp["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("Anthropic API error ({}): {}", status, msg);
        }

        resp["content"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .context(format!("Unexpected Anthropic API response: {}", resp))
    }

    async fn call_gemini(&self, text: &str) -> Result<String> {
        let body = json!({
            "system_instruction": {"parts": [{"text": SYSTEM_PROMPT}]},
            "contents": [{"parts": [{"text": text}]}]
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
            self.api_key
        );

        let http_resp = self.http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Gemini API request failed")?;

        let status = http_resp.status();
        let resp: serde_json::Value = http_resp.json().await?;

        if !status.is_success() {
            let msg = resp["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("Gemini API error ({}): {}", status, msg);
        }

        resp["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .context(format!("Unexpected Gemini API response: {}", resp))
    }

    async fn call_openai(&self, text: &str) -> Result<String> {
        let body = json!({
            "model": "gpt-4o-mini",
            "max_tokens": 1024,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": text}
            ]
        });

        let http_resp = self.http
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("OpenAI API request failed")?;

        let status = http_resp.status();
        let resp: serde_json::Value = http_resp.json().await?;

        if !status.is_success() {
            let msg = resp["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("OpenAI API error ({}): {}", status, msg);
        }

        resp["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .context(format!("Unexpected OpenAI API response: {}", resp))
    }
}

/// ローカル整形: ANSI ストリップ + 末尾 2000 文字に切り詰め
fn local_format(text: &str) -> String {
    let stripped = String::from_utf8_lossy(&strip_ansi_escapes::strip(text.as_bytes())).to_string();
    let char_count = stripped.chars().count();
    if char_count > 2000 {
        let skip = char_count - 2000;
        let offset = stripped.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
        format!("...\n{}", &stripped[offset..])
    } else {
        stripped
    }
}
