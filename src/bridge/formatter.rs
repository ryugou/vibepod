// LLM-based text formatter for cleaning TUI output before Slack notification

use anyhow::{bail, Context, Result};
use serde_json::json;

#[derive(Debug, Clone, PartialEq)]
pub enum LlmProvider {
    Anthropic,
    Gemini,
    OpenAi,
}

impl LlmProvider {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "gemini" | "google" => Ok(Self::Gemini),
            "openai" | "gpt" => Ok(Self::OpenAi),
            _ => bail!("Unknown LLM provider: '{}'. Use: anthropic, gemini, or openai", s),
        }
    }

    pub fn env_key_name(&self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
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
                truncate_raw(raw_text)
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
        }
    }

    async fn call_anthropic(&self, text: &str) -> Result<String> {
        let body = json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 1024,
            "system": SYSTEM_PROMPT,
            "messages": [{"role": "user", "content": text}]
        });

        let resp: serde_json::Value = self.http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("Anthropic API request failed")?
            .json()
            .await?;

        resp["content"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .context("Unexpected Anthropic API response format")
    }

    async fn call_gemini(&self, text: &str) -> Result<String> {
        let body = json!({
            "system_instruction": {"parts": [{"text": SYSTEM_PROMPT}]},
            "contents": [{"parts": [{"text": text}]}]
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key={}",
            self.api_key
        );

        let resp: serde_json::Value = self.http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Gemini API request failed")?
            .json()
            .await?;

        resp["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .context("Unexpected Gemini API response format")
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

        let resp: serde_json::Value = self.http
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("OpenAI API request failed")?
            .json()
            .await?;

        resp["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .context("Unexpected OpenAI API response format")
    }
}

/// LLM 失敗時のフォールバック: 末尾 2000 文字に切り詰め
fn truncate_raw(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count > 2000 {
        let skip = char_count - 2000;
        let offset = text.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
        format!("...\n{}", &text[offset..])
    } else {
        text.to_string()
    }
}
