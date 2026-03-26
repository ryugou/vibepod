# Security Hardening & Documentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address security review findings — add data transmission transparency, fix API error leaks, add `--llm none` mode, update docs to match implementation, and clean up dead code.

**Architecture:** Documentation-first approach (SECURITY.md, README.md updates) followed by code changes (`--llm none` mode, error sanitization, dead code removal). Each task is independently testable.

**Tech Stack:** Rust (clap, reqwest, serde_json), Markdown documentation

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `SECURITY.md` | Data transmission policy, trust model, reporting |
| Modify | `README.md` | Bridge docs, fix stale flags/paths, security model update |
| Modify | `src/bridge/formatter.rs:7-11,115-145` | Add `LlmProvider::None`, local-only formatting |
| Modify | `src/cli/mod.rs` | `--llm-provider` default docs |
| Modify | `src/cli/run.rs:280-298` | Allow `none` provider to skip API key validation |
| Modify | `src/cli/mod.rs:54` | Update `--llm-provider` help text to include `none` |
| Modify | `src/bridge/detector.rs:4-8` | Remove unused `Idle` variant |
| Modify | `Cargo.toml:28` | Remove chrono `serde` feature |
| Modify | `tests/bridge_detector_test.rs` | Update if `Idle` removal affects tests |
| Create | `tests/bridge_formatter_test.rs` | Tests for `LlmProvider::None` formatting |

---

### Task 1: SECURITY.md — Data Transmission & Trust Model

**Files:**
- Create: `SECURITY.md`

- [ ] **Step 1: Create SECURITY.md**

```markdown
# Security

## Reporting Vulnerabilities

Please report security vulnerabilities via GitHub Issues at <https://github.com/ryugou/vibepod/issues> with the `security` label, or email the maintainer directly.

## Data Transmission

VibePod operates in two modes with different data flows:

### Standard mode (`vibepod run`)

No external data transmission beyond the Docker container. Container communicates only with Claude's API (via the mounted auth token).

### Bridge mode (`vibepod run --bridge`)

Bridge mode sends data to **three external services**:

| Destination | What is sent | Why |
|---|---|---|
| **Slack** (via Bot/App tokens) | LLM-formatted terminal output excerpts, session start/end notifications | Remote notification when the agent is waiting for input |
| **LLM API** (Anthropic, Google, or OpenAI — selected via `--llm-provider`) | Raw terminal output (ANSI-stripped, last ~3000 chars) + a fixed system prompt | Cleans TUI artifacts before Slack notification |
| **Local disk** (`~/.config/vibepod/bridge-logs/`) | JSONL logs with terminal excerpts and stdin responses | Debugging and audit trail |

**What may leak:**
- Code snippets, file paths, prompts, or secrets that appear in the terminal output may be sent to the selected LLM provider and Slack.
- Each LLM provider has its own data retention and training policy. Review their terms before use.
- Use `--llm-provider none` to disable external LLM calls entirely (local ANSI stripping only).

**Startup disclosure:** VibePod prints a notice at bridge startup listing the active LLM provider and Slack channel.

### Gemini API key transport

The Gemini API uses a query-string `?key=` parameter (Google's official pattern). While functional, this means the API key appears in URLs. Proxy or network logs may capture it. The other providers (Anthropic, OpenAI) send keys via HTTP headers.

## Trust Model

### Slack channel security

In bridge mode, **anyone in the configured Slack channel** can respond to VibePod notifications (button clicks, reactions, thread replies). These responses are sent directly to the container's stdin.

**Recommendation:** Use a **private channel** with restricted membership. A shared public channel allows anyone in the workspace to send input to your container.

### bridge-logs

Log files at `~/.config/vibepod/bridge-logs/*.jsonl` contain terminal output excerpts and stdin input. File permissions are set to `0600` (owner-only). These files may contain sensitive information — treat them accordingly.

### Authentication

OAuth tokens are stored at `~/.config/vibepod/auth/token.json` with `0600` permissions. The OAuth callback opens a browser URL from Claude's auth flow.

## Container Isolation

See [README.md](README.md) for the 3-layer isolation model (Docker container, minimal mounts, git safety net).
```

- [ ] **Step 2: Verify file renders correctly**

Run: `head -5 SECURITY.md`
Expected: `# Security` header visible

- [ ] **Step 3: Commit**

```bash
git add SECURITY.md
git commit -m "docs: add SECURITY.md with data transmission and trust model"
```

---

### Task 2: README.md — Update to Match Implementation

**Files:**
- Modify: `README.md`

Changes needed:
1. `credentials.json` → `token.json` (line 36)
2. Remove `--isolated` and `--name` from run options table (lines 76-77)
3. Add `--bridge`, `--notify-delay`, `--slack-channel`, `--llm-provider` to run options table
4. Add bridge mode section with usage example
5. Update Security Model section to mention bridge data transmission
6. Update Roadmap (v1.3 bridge mode shipped)

- [ ] **Step 1: Fix login path**

Replace in README.md:
```
credentials.json
```
with:
```
token.json
```

- [ ] **Step 2: Update run options table**

Replace the current run options table with:

```markdown
| Option | Description |
|--------|-------------|
| *(none)* | **Interactive mode** — opens a Claude Code session inside the container |
| `--prompt "..."` | Fire-and-forget mode — agent runs autonomously and exits when done |
| `--resume` | Continue from the previous session (fire-and-forget) |
| `--no-network` | Disable container networking |
| `--env KEY=VALUE` | Pass environment variables (repeatable) |
| `--env-file <path>` | Load environment variables from file (`op://` references resolved via 1Password CLI) |
| `--bridge` | Enable Slack bridge mode (see below) |
| `--notify-delay <secs>` | Idle detection delay in seconds (default: 30, requires `--bridge`) |
| `--slack-channel <id>` | Override Slack channel ID from bridge.env |
| `--llm-provider <name>` | LLM for TUI output formatting: `anthropic` (default), `gemini`, `openai`, or `none` |
```

- [ ] **Step 3: Add Bridge Mode section after "Passing secrets with 1Password"**

```markdown
### Bridge Mode (Slack notifications)

Bridge mode monitors the container's terminal output and sends Slack notifications when the agent is waiting for input. You can respond directly from Slack.

```bash
vibepod run --bridge --llm-provider gemini
```

**Setup:**

1. Create a Slack app with Socket Mode, Bot Token Scopes (`chat:write`, `reactions:read`), and Event Subscriptions (`message.im`, `reaction_added`)
2. Configure `~/.config/vibepod/bridge.env`:

```
SLACK_BOT_TOKEN="xoxb-..."
SLACK_APP_TOKEN="xapp-..."
SLACK_CHANNEL_ID="C0123456789"
ANTHROPIC_API_KEY="sk-..."
GEMINI_API_KEY="AIza..."
OPENAI_API_KEY="sk-..."
```

Values can use `op://` references for 1Password integration.

3. Run with `--bridge`:

```bash
vibepod run --bridge                          # default: anthropic
vibepod run --bridge --llm-provider none      # no LLM, local ANSI stripping only
vibepod run --bridge --notify-delay 10        # 10s idle threshold
```

**Privacy:** Bridge mode sends terminal output to the selected LLM API and Slack. See [SECURITY.md](SECURITY.md) for details.
```

- [ ] **Step 4: Update Security Model section**

Replace current Security Model with:

```markdown
## Security Model

VibePod provides 3-layer isolation:

1. **Docker container** — the agent runs in an ephemeral container, not on your host
2. **Minimal mounts** — only your project directory and Claude auth are mounted; no `~/.ssh`, no `.env`, no home directory
3. **Git safety net** — your project is git-managed, so any unwanted changes can be reverted with `git reset --hard`

This follows [Anthropic's official recommendation](https://docs.anthropic.com/en/docs/claude-code/security) to use `--dangerously-skip-permissions` only inside containers.

**Bridge mode** adds external data transmission to Slack and an LLM API. See [SECURITY.md](SECURITY.md) for the full data flow and trust model.
```

- [ ] **Step 5: Update Roadmap**

Replace roadmap table with:

```markdown
| Version | Features |
|---------|----------|
| **v1.0** | `init` + `run` (interactive / fire-and-forget), Claude Code support |
| **v1.1** | Pre-installed plugins (superpowers, frontend-design), `--env-file` with 1Password integration |
| **v1.2** | `vibepod restore` (git HEAD auto-recovery with session reports) |
| **v1.3** | Slack bridge mode (`--bridge`), multi-provider LLM formatting |
| **v2** | Dashboard (Web UI), execution logs, progress monitoring |
| **v2.1+** | Gemini CLI / Codex support, multi-container execution |
```

- [ ] **Step 6: Verify README renders**

Run: `grep -c "credentials.json" README.md` — expected: 0
Run: `grep -c "isolated" README.md` — expected: 0
Run: `grep -c "bridge" README.md` — expected: >0

- [ ] **Step 7: Commit**

```bash
git add README.md
git commit -m "docs: update README to match current implementation (bridge, auth path, flags)"
```

---

### Task 3: `--llm-provider none` — Local-Only Formatting

> Note: The spec mentions `--bridge-no-llm` as an alternative. We implement `--llm-provider none` instead — it uses the existing flag infrastructure and avoids adding a redundant boolean flag. `none` / `local` are accepted as aliases.

**Files:**
- Modify: `src/bridge/formatter.rs:7-11,23-29,56-71`
- Modify: `src/cli/run.rs:280-298`
- Modify: `src/cli/mod.rs` (help text)
- Create: `tests/bridge_formatter_test.rs`

- [ ] **Step 1: Write failing test for `LlmProvider::None`**

Create `tests/bridge_formatter_test.rs`:

```rust
use vibepod::bridge::formatter::LlmProvider;

#[test]
fn test_llm_provider_none_from_str() {
    let provider = LlmProvider::from_str("none").unwrap();
    assert_eq!(provider, LlmProvider::None);
}

#[test]
fn test_llm_provider_none_env_key_is_none() {
    let provider = LlmProvider::None;
    assert_eq!(provider.env_key_name(), None);
}

#[tokio::test]
async fn test_formatter_none_strips_ansi() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("\x1b[31mred text\x1b[0m and normal").await;
    assert!(result.contains("red text"));
    assert!(result.contains("and normal"));
    assert!(!result.contains("\x1b["));
}

#[tokio::test]
async fn test_formatter_none_truncates_long_text() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let long_text = "x".repeat(3000);
    let result = formatter.format(&long_text).await;
    assert!(result.chars().count() <= 2001); // 2000 + possible "..."
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test bridge_formatter_test 2>&1 | tail -5`
Expected: Compilation error — `LlmProvider::None` doesn't exist yet

- [ ] **Step 3: Add `None` variant to `LlmProvider`**

In `src/bridge/formatter.rs`, add `None` variant and update methods:

```rust
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
```

Update `format()` to handle `None` provider:

```rust
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
```

Add `local_format` function (rename from `truncate_raw`). Note: the error fallback also switches to `local_format`, which now strips ANSI — this is an intentional improvement over the previous `truncate_raw` which left ANSI in the fallback text:

```rust
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
```

Add `None` arm to `call_llm` match (around line 83-87) to satisfy exhaustive match:

```rust
async fn call_llm(&self, text: &str) -> Result<String> {
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
```

- [ ] **Step 4: Update `env_key_name()` call sites in `src/cli/run.rs`**

The return type changes from `&str` to `Option<&str>`. Update validation (around line 280-298):

```rust
// LLM provider & API key
let provider = crate::bridge::formatter::LlmProvider::from_str(&llm_provider)?;
let llm_api_key = provider
    .env_key_name()
    .and_then(|key| resolved.get(key).cloned())
    .unwrap_or_default();

// Validation
let mut missing = Vec::new();
if slack_bot_token.is_empty() {
    missing.push("SLACK_BOT_TOKEN".to_string());
}
if slack_app_token.is_empty() {
    missing.push("SLACK_APP_TOKEN".to_string());
}
if slack_channel_id.is_empty() {
    missing.push("SLACK_CHANNEL_ID (set via --slack-channel or in bridge.env)".to_string());
}
if let Some(key_name) = provider.env_key_name() {
    if llm_api_key.is_empty() {
        missing.push(key_name.to_string());
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --test bridge_formatter_test 2>&1 | tail -10`
Expected: All 4 tests pass

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass (no regressions)

- [ ] **Step 6: Commit**

- [ ] **Step 5b: Update `src/cli/mod.rs` help text**

In `src/cli/mod.rs`, update the `--llm-provider` arg's help/doc attribute to include `none`:

Change the current help text from:
```
"LLM provider for formatting notifications: anthropic (default), gemini, openai"
```
to:
```
"LLM provider for formatting notifications: anthropic (default), gemini, openai, none"
```

- [ ] **Step 6: Run tests**

Run: `cargo test --test bridge_formatter_test 2>&1 | tail -10`
Expected: All 4 tests pass

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass (no regressions)

- [ ] **Step 7: Commit**

```bash
git add src/bridge/formatter.rs src/cli/run.rs src/cli/mod.rs tests/bridge_formatter_test.rs
git commit -m "feat: add --llm-provider none for local-only formatting without API key"
```

---

### Task 4: Bridge Startup Disclosure Message

**Files:**
- Modify: `src/cli/run.rs:309` (the existing bridge mode enabled line)

- [ ] **Step 1: Update startup message**

Replace the existing line (around line 309):
```rust
println!("  ◇  Bridge mode enabled (notify delay: {}s, llm: {:?})", notify_delay, provider);
```

With:
```rust
println!("  ◇  Bridge mode enabled (notify delay: {}s, llm: {:?})", notify_delay, provider);
if provider != crate::bridge::formatter::LlmProvider::None {
    println!("  │  ⚠ Terminal output excerpts will be sent to {:?} API and Slack for formatting.", provider);
} else {
    println!("  │  Terminal output will be sent to Slack (local formatting, no LLM API calls).");
}
```

- [ ] **Step 2: Build and verify**

Run: `cargo build --release 2>&1 | tail -3`
Expected: Build succeeds

- [ ] **Step 3: Commit**

```bash
git add src/cli/run.rs
git commit -m "feat: show data transmission notice at bridge startup"
```

---

### Task 5: Sanitize API Error Messages

**Files:**
- Modify: `src/bridge/formatter.rs:118,150,182` (three `context(format!(...))` calls)

- [ ] **Step 1: Replace verbose error context with safe messages**

In `call_anthropic`, `call_gemini`, `call_openai` — change the final `.context(format!("Unexpected ... response: {}", resp))` to short messages without the full response body:

```rust
// call_anthropic:
.context("Unexpected Anthropic API response: missing content[0].text")

// call_gemini:
.context("Unexpected Gemini API response: missing candidates[0].content.parts[0].text")

// call_openai:
.context("Unexpected OpenAI API response: missing choices[0].message.content")
```

- [ ] **Step 2: Build and verify**

Run: `cargo build --release 2>&1 | tail -3`
Expected: Build succeeds

- [ ] **Step 3: Commit**

```bash
git add src/bridge/formatter.rs
git commit -m "fix: remove full API response from error messages to prevent data leakage"
```

---

### Task 6: Remove Unused `DetectorState::Idle`

**Files:**
- Modify: `src/bridge/detector.rs:4-8`
- Verify: `tests/bridge_detector_test.rs` (no references to `Idle`)

- [ ] **Step 1: Verify `Idle` is unused**

Run: `grep -r "Idle" src/ tests/ --include="*.rs" | grep -v "IdleDetector\|check_idle\|notify_idle\|idle_notification\|idle_detection"`
Expected: Only `DetectorState::Idle` definition and the `use` in `mod.rs`

- [ ] **Step 2: Remove `Idle` variant**

In `src/bridge/detector.rs`, change:
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum DetectorState {
    Buffering,
    Idle,
    WaitingResponse,
}
```
to:
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum DetectorState {
    Buffering,
    WaitingResponse,
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/bridge/detector.rs
git commit -m "refactor: remove unused DetectorState::Idle variant"
```

---

### Task 7: Slim Down chrono Features

**Files:**
- Modify: `Cargo.toml:28`

- [ ] **Step 1: Check if `serde` feature is needed**

`chrono` serde feature is only needed if chrono types are (de)serialized via serde. Current usage is only `Local::now().to_rfc3339()` and `Utc::now().to_rfc3339()` — these are `Display`/`format` methods, not serde.

Verify: `grep -r "chrono.*Serialize\|chrono.*Deserialize\|#\[serde" src/ | grep -i chrono`
Expected: No matches

- [ ] **Step 2: Remove serde feature from chrono**

In `Cargo.toml`, change:
```toml
chrono = { version = "0.4", features = ["serde"] }
```
to:
```toml
chrono = "0.4"
```

- [ ] **Step 3: Build and run tests**

Run: `cargo build --release 2>&1 | tail -3`
Expected: Build succeeds

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml
git commit -m "chore: remove unused chrono serde feature"
```

---

### Task 8: Slack message_ts Verification Comment

**Files:**
- Modify: `src/bridge/slack.rs:287-343`

- [ ] **Step 1: Add documentation comments to handle_interaction, handle_reaction, handle_message**

Add clarifying comments about ts semantics:

In `handle_interaction`:
```rust
fn handle_interaction(&self, envelope: &Value) -> Option<SlackResponse> {
    let payload = &envelope["payload"];
    let actions = payload["actions"].as_array()?;
    let action = actions.first()?;
    let action_id = action["action_id"].as_str()?;
    // message.ts identifies the notification message this button belongs to.
    // Used by update_responded() to replace the notification with "responded" status.
    let message_ts = payload["message"]["ts"].as_str()?;
    ...
```

In `handle_reaction`:
```rust
fn handle_reaction(&self, event: &Value) -> Option<SlackResponse> {
    let reaction = event["reaction"].as_str()?;
    // item.ts is the message the reaction was added to.
    // This should match a notification we sent; unrelated reactions are harmless
    // (they produce a SlackResponse but update_responded will target our message).
    let message_ts = event["item"]["ts"].as_str()?;
    ...
```

In `handle_message`:
```rust
fn handle_message(&self, event: &Value) -> Option<SlackResponse> {
    ...
    let (source, message_ts) = if let Some(ts) = thread_ts {
        // Thread reply: thread_ts is the parent notification message.
        // Used by update_responded() to mark the original notification as handled.
        ("slack_thread", ts.to_string())
    } else {
        // Direct message (not in a thread): ts is the message itself.
        // update_responded() targets this message — which is the DM, not a notification.
        // This is acceptable for DM-only flows.
        ("slack_dm", ts)
    };
    ...
```

- [ ] **Step 2: Build**

Run: `cargo build --release 2>&1 | tail -3`
Expected: Build succeeds

- [ ] **Step 3: Commit**

```bash
git add src/bridge/slack.rs
git commit -m "docs: clarify message_ts/thread_ts semantics in Slack handlers"
```
