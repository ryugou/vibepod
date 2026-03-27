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

impl std::str::FromStr for LlmProvider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "gemini" | "google" => Ok(Self::Gemini),
            "openai" | "gpt" => Ok(Self::OpenAi),
            "none" | "local" => Ok(Self::None),
            _ => bail!(
                "Unknown LLM provider: '{}'. Use: anthropic, gemini, openai, or none",
                s
            ),
        }
    }
}

impl LlmProvider {
    pub fn env_key_name(&self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Gemini => Some("GEMINI_API_KEY"),
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::None => None,
        }
    }
}

/// LLM 整形結果: テキスト + 検出した選択肢
#[derive(Debug, Clone, PartialEq)]
pub struct FormatResult {
    pub text: String,
    pub choices: Vec<String>,
}

/// テキストから選択肢パターンを検出する
pub fn detect_choices(text: &str) -> Vec<String> {
    // 1. yes/no パターン: (y/n), (yes/no)
    if regex::Regex::new(r"(?i)\(y(?:es)?/n(?:o)?\)")
        .unwrap()
        .is_match(text)
    {
        return vec!["yes".to_string(), "no".to_string()];
    }

    // 2. アルファベット選択肢: "A) ...", "A: ...", "A - ...", "- A: ...", "- A) ..."
    let alpha_re = regex::Regex::new(r"(?m)^[\s\-*]*([A-Z])\s*[):\-]").unwrap();
    let alpha_choices: Vec<String> = alpha_re
        .captures_iter(text)
        .map(|cap| cap[1].to_string())
        .collect();
    if alpha_choices.len() >= 2 {
        return alpha_choices;
    }

    // 3. 数字選択肢: "1. ...", "1) ..."
    let num_re = regex::Regex::new(r"(?m)^[\s\-*]*(\d+)\s*[).\-]").unwrap();
    let num_choices: Vec<String> = num_re
        .captures_iter(text)
        .map(|cap| cap[1].to_string())
        .collect();
    if num_choices.len() >= 2 {
        return num_choices;
    }

    vec![]
}

const SYSTEM_PROMPT: &str = "\
You are a text extraction tool. Given raw terminal output from a TUI application (Claude Code), \
extract the meaningful text content and detect any choices being presented to the user.\n\
\n\
## Rules\n\
1. Remove ONLY UI decorations: box-drawing characters, spinner text, prompt symbols (❯ ● ✶), \
status indicators, shortcut hints, progress bars, and TUI artifacts.\n\
2. Preserve the COMPLETE text content — do NOT summarize, truncate, or omit any part. \
Every sentence, every choice description, and every question must be included verbatim.\n\
3. Preserve the ORIGINAL LANGUAGE of the text. If the text is in Japanese, the output must be in Japanese. \
Never translate.\n\
4. When multiple questions exist in the text, include ALL of them in the \"text\" field.\n\
\n\
## Output format\n\
Return a JSON object with exactly two fields:\n\
- \"text\": the cleaned text content (string). Must contain the full original text without omission.\n\
- \"choices\": detected choice LABELS ONLY as short strings. Examples:\n\
  - Yes/No prompt (y/n): [\"yes\", \"no\"]\n\
  - Letter options a)/b)/c) or A/B/C: [\"a\", \"b\", \"c\"]\n\
  - Numbered options 1./2./3.: [\"1\", \"2\", \"3\"]\n\
  - No choices detected: []\n\
  IMPORTANT: choices must contain ONLY the label (\"a\", \"b\", \"1\", \"yes\"), NOT the full description text.\n\
\n\
If the input is just UI noise with no meaningful content, return: {\"text\": \"[no content]\", \"choices\": []}\n\
Return ONLY the JSON object, no markdown fences or explanations.";

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
    pub async fn format(&self, raw_text: &str) -> FormatResult {
        if self.provider == LlmProvider::None {
            let text = local_format(raw_text);
            let choices = detect_choices(&text);
            return FormatResult { text, choices };
        }
        match self.call_llm(raw_text).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!("Warning: LLM formatting failed, using raw text: {}", e);
                let text = local_format(raw_text);
                let choices = detect_choices(&text);
                FormatResult { text, choices }
            }
        }
    }

    async fn call_llm(&self, text: &str) -> Result<FormatResult> {
        // NOTE: API コスト・レイテンシ抑制のため末尾 3000 文字に切り詰める。
        // SYSTEM_PROMPT は「全文保持」を指示しているが、ここで既に先頭が落ちる場合がある。
        // また max_tokens: 1024 のため、LLM の出力側でも長文は切り詰められる可能性がある。
        let input = if text.chars().count() > 3000 {
            let skip = text.chars().count() - 3000;
            let offset = text.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
            &text[offset..]
        } else {
            text
        };

        let raw = match self.provider {
            LlmProvider::Anthropic => self.call_anthropic(input).await?,
            LlmProvider::Gemini => self.call_gemini(input).await?,
            LlmProvider::OpenAi => self.call_openai(input).await?,
            LlmProvider::None => unreachable!("None provider handled in format()"),
        };

        // LLM レスポンスを JSON としてパース。失敗時は旧形式（プレーンテキスト）としてフォールバック
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(val) => {
                let mut text = val["text"].as_str().unwrap_or(&raw).to_string();
                if text.trim() == "[no content]" {
                    text = String::new();
                }
                let choices = val["choices"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(FormatResult { text, choices })
            }
            Err(_) => {
                // LLM がプレーンテキストを返した場合のフォールバック
                let mut text = raw;
                if text.trim() == "[no content]" {
                    text = String::new();
                }
                let choices = detect_choices(&text);
                Ok(FormatResult { text, choices })
            }
        }
    }

    async fn call_anthropic(&self, text: &str) -> Result<String> {
        let body = json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 1024,
            "system": SYSTEM_PROMPT,
            "messages": [{"role": "user", "content": text}]
        });

        let http_resp = self
            .http
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
            .context("Unexpected Anthropic API response: missing content[0].text")
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

        let http_resp = self
            .http
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
            .context("Unexpected Gemini API response: missing candidates[0].content.parts[0].text")
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

        let http_resp = self
            .http
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
            .context("Unexpected OpenAI API response: missing choices[0].message.content")
    }
}

/// ローカル整形: ANSI ストリップ + 末尾 2000 文字に切り詰め。
/// LLM 経路（3000 文字 + LLM 要約）とは切り詰め基準が異なる。
/// LLM 未使用時・フォールバック時のベストエフォート表示用。
fn local_format(text: &str) -> String {
    let stripped = String::from_utf8_lossy(&strip_ansi_escapes::strip(text.as_bytes())).to_string();
    let char_count = stripped.chars().count();
    if char_count > 2000 {
        let skip = char_count - 2000;
        let offset = stripped
            .char_indices()
            .nth(skip)
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("...\n{}", &stripped[offset..])
    } else {
        stripped
    }
}
