# Dynamic Slack Buttons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Slack 通知のボタンを端末出力の選択肢に応じて動的に生成する（Yes/No → yes/no ボタン、A/B/C → A/B/C ボタン、検出なし → ボタンなし）

**Architecture:** `Formatter::format()` の返り値を `FormatResult { text, choices }` に変更。LLM プロバイダーには JSON で選択肢を返させ、`None` プロバイダーでは正規表現で検出。Slack の `build_idle_notification_blocks` を choices に応じた動的ボタン生成に改修。`map_action_to_stdin` を動的 action_id 対応に。

**Tech Stack:** Rust (regex, serde_json), Slack Block Kit

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/bridge/formatter.rs` | `FormatResult` 構造体追加、LLM プロンプト変更（JSON 返却）、`None` 用正規表現検出 |
| Modify | `src/bridge/slack.rs:392-451` | `build_idle_notification_blocks` を choices 対応に、`map_action_to_stdin` を動的に |
| Modify | `src/bridge/mod.rs:207-219` | `format()` の返り値を `FormatResult` に合わせて変更 |
| Modify | `tests/bridge_formatter_test.rs` | `FormatResult` 対応 + 選択肢検出テスト追加 |
| Modify | `tests/bridge_slack_test.rs` | 動的ボタン生成テスト追加 |

---

### Task 1: `FormatResult` 構造体 & 選択肢検出（formatter.rs）

**Files:**
- Modify: `src/bridge/formatter.rs`
- Modify: `tests/bridge_formatter_test.rs`

- [ ] **Step 1: Write failing tests for `FormatResult` and choice detection**

Append to `tests/bridge_formatter_test.rs`:

```rust
use vibepod::bridge::formatter::{FormatResult, detect_choices};

#[test]
fn test_detect_choices_yes_no() {
    let choices = detect_choices("Do you want to proceed? (y/n)");
    assert_eq!(choices, vec!["yes".to_string(), "no".to_string()]);
}

#[test]
fn test_detect_choices_yes_no_full() {
    let choices = detect_choices("Continue? (yes/no)");
    assert_eq!(choices, vec!["yes".to_string(), "no".to_string()]);
}

#[test]
fn test_detect_choices_abc_paren() {
    let choices = detect_choices("Choose:\nA) Apple\nB) Banana\nC) Cherry");
    assert_eq!(choices, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
}

#[test]
fn test_detect_choices_abc_dash() {
    let choices = detect_choices("- A: Apple\n- B: Banana\n- C: Cherry");
    assert_eq!(choices, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
}

#[test]
fn test_detect_choices_abc_hyphen() {
    let choices = detect_choices("A - Apple\nB - Banana\nC - Cherry");
    assert_eq!(choices, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
}

#[test]
fn test_detect_choices_none() {
    let choices = detect_choices("This is just a status message with no choices.");
    assert!(choices.is_empty());
}

#[test]
fn test_detect_choices_numbered() {
    let choices = detect_choices("Options:\n1. First\n2. Second\n3. Third");
    assert_eq!(choices, vec!["1".to_string(), "2".to_string(), "3".to_string()]);
}

#[tokio::test]
async fn test_formatter_none_returns_format_result() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("Choose:\nA) Apple\nB) Banana").await;
    assert!(!result.text.is_empty());
    assert_eq!(result.choices, vec!["A", "B"]);
}

#[tokio::test]
async fn test_formatter_none_yes_no() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("Proceed? (y/n)").await;
    assert_eq!(result.choices, vec!["yes", "no"]);
}

#[tokio::test]
async fn test_formatter_none_no_choices() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("Processing files...").await;
    assert!(result.choices.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test bridge_formatter_test 2>&1 | tail -5`
Expected: Compilation error — `FormatResult` and `detect_choices` don't exist

- [ ] **Step 3: Add `FormatResult` struct and `detect_choices` function**

In `src/bridge/formatter.rs`, add after the `LlmProvider` impl block (after line 33):

```rust
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
```

- [ ] **Step 4: Change `format()` return type to `FormatResult`**

Replace the `format()` method:

```rust
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
```

- [ ] **Step 5: Change `call_llm` and provider methods to return `FormatResult`**

Update the LLM system prompt (replace the existing `SYSTEM_PROMPT` const):

```rust
const SYSTEM_PROMPT: &str = "\
You are a text extraction tool. Given raw terminal output from a TUI application (Claude Code), \
extract the meaningful text content and detect any choices being presented to the user.\n\
Remove all UI decorations: box-drawing characters, spinner text, prompt symbols (❯ ● ✶), \
status indicators, shortcut hints, and any other TUI artifacts.\n\
\n\
Return a JSON object with exactly two fields:\n\
- \"text\": the cleaned text content (string)\n\
- \"choices\": detected choices as an array of strings. Examples:\n\
  - Yes/No prompt (y/n): [\"yes\", \"no\"]\n\
  - Letter options A/B/C: [\"A\", \"B\", \"C\"]\n\
  - Numbered options 1/2/3: [\"1\", \"2\", \"3\"]\n\
  - No choices detected: []\n\
\n\
If the input is just UI noise with no meaningful content, return: {\"text\": \"[no content]\", \"choices\": []}\n\
Return ONLY the JSON object, no markdown fences or explanations.";
```

Update `call_llm` to parse the JSON response:

```rust
async fn call_llm(&self, text: &str) -> Result<FormatResult> {
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
            // [no content] → 空文字列（mod.rs で通知��キップに使う）
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
```

Note: `call_anthropic`, `call_gemini`, `call_openai` の返り値は `Result<String>` のまま変更不要。JSON パースは `call_llm` で一元処理。

- [ ] **Step 6: Update existing tests for new return type**

In `tests/bridge_formatter_test.rs`, update existing tests:

```rust
// test_formatter_none_strips_ansi — change result.contains to result.text.contains
#[tokio::test]
async fn test_formatter_none_strips_ansi() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("\x1b[31mred text\x1b[0m and normal").await;
    assert!(result.text.contains("red text"));
    assert!(result.text.contains("and normal"));
    assert!(!result.text.contains("\x1b["));
}

// test_formatter_none_truncates_long_text — change result.chars() to result.text.chars()
#[tokio::test]
async fn test_formatter_none_truncates_long_text() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let long_text = "x".repeat(3000);
    let result = formatter.format(&long_text).await;
    assert!(result.text.chars().count() <= 2005);
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test --test bridge_formatter_test 2>&1 | tail -15`
Expected: All tests pass

- [ ] **Step 8: Commit**

```bash
git add src/bridge/formatter.rs tests/bridge_formatter_test.rs
git commit -m "feat: add FormatResult with choice detection for dynamic Slack buttons"
```

---

### Task 2: Dynamic Slack Button Generation (slack.rs)

**Files:**
- Modify: `src/bridge/slack.rs:392-451`
- Modify: `tests/bridge_slack_test.rs`

- [ ] **Step 1: Write failing tests for dynamic buttons**

Append to `tests/bridge_slack_test.rs`:

```rust
#[test]
fn test_dynamic_buttons_yes_no() {
    let choices = vec!["yes".to_string(), "no".to_string()];
    let blocks = build_idle_notification_blocks("Proceed? (y/n)", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    let actions = blocks.iter().find(|b| b["type"] == "actions").expect("actions block");
    let elements = actions["elements"].as_array().unwrap();
    // yes, no, Skip の3ボタン
    assert_eq!(elements.len(), 3);
    assert_eq!(elements[0]["action_id"], "respond_choice_yes");
    assert_eq!(elements[0]["text"]["text"], "Yes");
    assert_eq!(elements[1]["action_id"], "respond_choice_no");
    assert_eq!(elements[1]["text"]["text"], "No");
    assert_eq!(elements[2]["action_id"], "respond_skip");
}

#[test]
fn test_dynamic_buttons_abc() {
    let choices = vec!["A".to_string(), "B".to_string(), "C".to_string()];
    let blocks = build_idle_notification_blocks("Choose:", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    let actions = blocks.iter().find(|b| b["type"] == "actions").expect("actions block");
    let elements = actions["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 4); // A, B, C, Skip
    assert_eq!(elements[0]["action_id"], "respond_choice_A");
    assert_eq!(elements[1]["action_id"], "respond_choice_B");
    assert_eq!(elements[2]["action_id"], "respond_choice_C");
    assert_eq!(elements[3]["action_id"], "respond_skip");
}

#[test]
fn test_dynamic_buttons_no_choices() {
    let choices: Vec<String> = vec![];
    let blocks = build_idle_notification_blocks("Status output", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    // ボタンなし — actions block が存在しない
    let actions = blocks.iter().find(|b| b["type"] == "actions");
    assert!(actions.is_none());
}

#[test]
fn test_dynamic_buttons_numbered() {
    let choices = vec!["1".to_string(), "2".to_string(), "3".to_string()];
    let blocks = build_idle_notification_blocks("Pick:", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    let actions = blocks.iter().find(|b| b["type"] == "actions").expect("actions block");
    let elements = actions["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 4); // 1, 2, 3, Skip
    assert_eq!(elements[0]["action_id"], "respond_choice_1");
    assert_eq!(elements[1]["action_id"], "respond_choice_2");
    assert_eq!(elements[2]["action_id"], "respond_choice_3");
    assert_eq!(elements[3]["action_id"], "respond_skip");
}

#[test]
fn test_map_action_dynamic_choice() {
    assert_eq!(map_action_to_stdin("respond_choice_A"), Some("A\r".to_string()));
    assert_eq!(map_action_to_stdin("respond_choice_yes"), Some("yes\r".to_string()));
    assert_eq!(map_action_to_stdin("respond_choice_no"), Some("no\r".to_string()));
    assert_eq!(map_action_to_stdin("respond_choice_1"), Some("1\r".to_string()));
    assert_eq!(map_action_to_stdin("respond_skip"), Some("\r".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test bridge_slack_test 2>&1 | tail -5`
Expected: Compilation error — signature changed

- [ ] **Step 3: Update `build_idle_notification_blocks` to accept choices**

Replace the function in `src/bridge/slack.rs`:

```rust
/// Block Kit メッセージ構造を構築（無音検知通知用）
/// choices が空の場合はボタンなし、ありの場合は選択肢 + Skip ボタン
pub fn build_idle_notification_blocks(
    buffer_content: &str,
    project_name: &str,
    session_id: &str,
    choices: &[String],
) -> Value {
    let section = json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": format!(
                "\u{1f514} *VibePod* [{}] (session: `{}`)\nセッション出力が停止しました\n```\n{}\n```",
                project_name, session_id, buffer_content
            )
        }
    });

    let context = json!({
        "type": "context",
        "elements": [{
            "type": "mrkdwn",
            "text": "スレッドに返信でテキスト入力も可能"
        }]
    });

    if choices.is_empty() {
        // 選択肢なし — ボタンなし、テキスト返信のみ
        return json!([section, context]);
    }

    // 選択肢ボタン生成
    let mut elements: Vec<Value> = choices
        .iter()
        .enumerate()
        .map(|(i, choice)| {
            let mut btn = json!({
                "type": "button",
                "text": { "type": "plain_text", "text": choice_display_text(choice) },
                "action_id": format!("respond_choice_{}", choice),
            });
            // 最初のボタンを primary スタイルに
            if i == 0 {
                btn["style"] = json!("primary");
            }
            // "no" ボタンを danger スタイルに
            if choice.to_lowercase() == "no" {
                btn["style"] = json!("danger");
            }
            btn
        })
        .collect();

    // Skip ボタンを末尾に追加
    elements.push(json!({
        "type": "button",
        "text": { "type": "plain_text", "text": "Skip" },
        "action_id": "respond_skip",
    }));

    let actions = json!({
        "type": "actions",
        "elements": elements,
    });

    json!([section, actions, context])
}

/// 選択肢の表示テキスト（yes → Yes, A → A）
fn choice_display_text(choice: &str) -> String {
    match choice {
        "yes" => "Yes".to_string(),
        "no" => "No".to_string(),
        _ => choice.to_string(),
    }
}
```

- [ ] **Step 4: Update `map_action_to_stdin` to handle dynamic action_ids**

Replace in `src/bridge/slack.rs`:

```rust
/// ボタン action_id → stdin テキストのマッピング
/// pty raw mode では Enter は \r（キャリッジリターン）
/// 動的ボタン: "respond_choice_X" → "X\r"
pub fn map_action_to_stdin(action_id: &str) -> Option<String> {
    if action_id == "respond_skip" {
        return Some("\r".to_string());
    }
    if let Some(choice) = action_id.strip_prefix("respond_choice_") {
        return Some(format!("{}\r", choice));
    }
    // レガシー互換（旧ボタンが残っている場合。Claude Code は y/n 単文字を期待）
    match action_id {
        "respond_yes" => Some("y\r".to_string()),
        "respond_no" => Some("n\r".to_string()),
        _ => None,
    }
}
```

- [ ] **Step 5: Update existing tests for new signature**

In `tests/bridge_slack_test.rs`, update existing tests that call `build_idle_notification_blocks` to pass `&choices` parameter:

```rust
// test_block_kit_message_structure — add default yes/no choices
#[test]
fn test_block_kit_message_structure() {
    let choices = vec!["yes".to_string(), "no".to_string()];
    let blocks = build_idle_notification_blocks(
        "Do you want to proceed? (y/n)",
        "my-project",
        "20260325-143000-a1b2",
        &choices,
    );
    // ... rest of test (update action_id checks to respond_choice_yes/respond_choice_no)
    let blocks = blocks.as_array().unwrap();
    assert!(blocks.len() >= 2);
    let section = blocks.iter().find(|b| b["type"] == "section").expect("section block");
    assert_eq!(section["text"]["type"], "mrkdwn");
    let text = section["text"]["text"].as_str().unwrap();
    assert!(text.contains("Do you want to proceed? (y/n)"));
    let actions = blocks.iter().find(|b| b["type"] == "actions").expect("actions block");
    let elements = actions["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 3); // yes, no, Skip
}

// test_block_kit_contains_project_and_session — add empty choices (no buttons)
#[test]
fn test_block_kit_contains_project_and_session() {
    let choices: Vec<String> = vec![];
    let blocks = build_idle_notification_blocks(
        "output text",
        "my-project",
        "20260325-143000-a1b2",
        &choices,
    );
    let blocks_str = serde_json::to_string(&blocks).unwrap();
    assert!(blocks_str.contains("my-project"));
    assert!(blocks_str.contains("20260325-143000-a1b2"));
}
```

Update old `map_action_to_stdin` tests — `respond_yes`/`respond_no` now send "yes\r"/"no\r" (not "y\r"/"n\r"):

```rust
#[test]
fn test_response_mapping_yes_legacy() {
    // レガシー互換: Claude Code は y/n 単文字を期待
    assert_eq!(map_action_to_stdin("respond_yes"), Some("y\r".to_string()));
}

#[test]
fn test_response_mapping_no_legacy() {
    assert_eq!(map_action_to_stdin("respond_no"), Some("n\r".to_string()));
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test --test bridge_slack_test 2>&1 | tail -15`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add src/bridge/slack.rs tests/bridge_slack_test.rs
git commit -m "feat: dynamic Slack buttons based on detected choices"
```

---

### Task 3: Wire FormatResult Through Bridge Main Loop (mod.rs)

**Files:**
- Modify: `src/bridge/mod.rs:207-219`
- Modify: `src/bridge/slack.rs:99` (`notify_idle` signature)

- [ ] **Step 1: Update `notify_idle` to accept choices**

In `src/bridge/slack.rs`, change `notify_idle` signature (around line 99):

```rust
pub async fn notify_idle(&self, buffer_content: &str, choices: &[String]) -> Result<String> {
    let blocks = build_idle_notification_blocks(
        buffer_content,
        &self.project_name,
        &self.session_id,
        choices,
    );
    // ... rest unchanged
```

- [ ] **Step 2: Update main loop in mod.rs to use FormatResult**

In `src/bridge/mod.rs`, update the idle check block (around line 207-219):

Replace:
```rust
let content = text_formatter.format(&raw_content).await;
if content.is_empty() {
```

With:
```rust
let format_result = text_formatter.format(&raw_content).await;
if format_result.text.is_empty() {
```

And update `notify_idle` call:
```rust
match notify_slack.notify_idle(&format_result.text, &format_result.choices).await {
```

And update `logger.log_notified`:
```rust
logger.log_notified(&format_result.text).ok();
```

- [ ] **Step 3: Build and run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass

Run: `cargo build --release 2>&1 | tail -3`
Expected: Build succeeds

- [ ] **Step 4: Commit**

```bash
git add src/bridge/mod.rs src/bridge/slack.rs
git commit -m "feat: wire FormatResult choices through bridge loop to Slack notifications"
```
