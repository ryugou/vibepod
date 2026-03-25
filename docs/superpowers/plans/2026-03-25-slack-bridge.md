# Slack Bridge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `vibepod run --bridge` で、コンテナ出力の無音検知 → Slack 通知 → Slack/ターミナルからの応答 → コンテナ stdin 送信 を実現する。

**Architecture:** bollard `attach_container` でコンテナの stdin/stdout ストリームを取得し、ホスト側ターミナルを raw mode にして透過転送。無音 N 秒検知で Slack Socket Mode 経由の通知を送り、ボタン/リアクション/スレッド返信で応答を受信して stdin に注入する。

**Tech Stack:** Rust, bollard (Docker API), tokio (async runtime), Slack Web API + Socket Mode (reqwest + tokio-tungstenite or slack-morphism), strip-ansi-escapes

**Spec:** `docs/superpowers/specs/2026-03-25-slack-bridge-design.md`

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `src/bridge/mod.rs` | bridge モードのエントリポイント。`pub async fn run()` を公開 |
| `src/bridge/io.rs` | bollard attach、ターミナル raw mode（RAII ガード）、stdin/stdout 転送、SIGWINCH リサイズ |
| `src/bridge/detector.rs` | 無音検知タイマー、出力バッファ管理、状態遷移（Buffering/Idle/WaitingResponse）、ANSI ストリップ |
| `src/bridge/slack.rs` | Slack Socket Mode 接続、通知送信（Block Kit）、応答受信（ボタン/リアクション/スレッド返信）、再接続 |
| `src/bridge/logger.rs` | notified/responded イベントの JSONL ログ記録 |
| `tests/bridge_detector_test.rs` | detector の状態遷移・バッファ管理テスト |
| `tests/bridge_logger_test.rs` | logger のイベント記録テスト |
| `tests/bridge_slack_test.rs` | Slack メッセージ構造・レスポンスマッピングテスト |

### Modified Files

| File | Change |
|------|--------|
| `src/cli/mod.rs` | `--bridge`, `--notify-delay`, `--slack-channel` オプション追加 |
| `src/main.rs` | `Commands::Run` の match パターンに `bridge`, `notify_delay`, `slack_channel` を追加 |
| `src/cli/run.rs` | `--bridge` 判定分岐（既存 interactive/fire-and-forget 分岐の**前**に配置、early return）、bridge.env 読み込み、`bridge::run()` 呼び出し。ContainerConfig 構築をヘルパー関数に抽出して bridge/fire-and-forget で共有 |
| `src/runtime/docker.rs` | `attach_container()`, `resize_container_tty()`, `wait_container()` メソッド追加 |
| `src/lib.rs` | `pub mod bridge;` 追加 |
| `Cargo.toml` | 依存追加（strip-ansi-escapes, tokio features, Slack クレート） |

### Existing Dependencies (追加不要)

`libc`, `serde`, `serde_json`, `chrono`, `tempfile`(dev) は既に Cargo.toml に含まれている。

---

## Task 1: CLI オプション追加と bridge.env 読み込み

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/cli/mod.rs:29-45`
- Modify: `src/main.rs:21-28` (Commands::Run の match パターン更新)
- Modify: `src/cli/run.rs:10-20` (引数受け取り)
- Modify: `src/lib.rs`
- Create: `src/bridge/mod.rs`

- [ ] **Step 1: Cargo.toml に依存追加**

tokio features に `"io-util"`, `"time"`, `"sync"` を追加。`strip-ansi-escapes = "0.2"` を追加。

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal", "io-util", "time", "sync"] }
strip-ansi-escapes = "0.2"
```

- [ ] **Step 2: cli/mod.rs に --bridge, --notify-delay, --slack-channel 追加**

```rust
Run {
    #[arg(long)]
    resume: bool,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    no_network: bool,
    #[arg(long, num_args = 1)]
    env: Vec<String>,
    #[arg(long)]
    env_file: Option<String>,
    // 新規追加
    #[arg(long)]
    bridge: bool,
    #[arg(long, default_value = "30")]
    notify_delay: u64,
    #[arg(long)]
    slack_channel: Option<String>,
},
```

- [ ] **Step 3: main.rs の match パターンと run.rs の execute() シグネチャを更新**

`src/main.rs` の `Commands::Run { ... }` デストラクチャリングに `bridge`, `notify_delay`, `slack_channel` を追加。`execute()` のシグネチャに新しい引数を追加。まだ bridge 分岐ロジックは入れない。

- [ ] **Step 4: bridge モジュールのスケルトン作成**

`src/bridge/mod.rs` を作成し、空の `pub async fn run()` を定義。`src/lib.rs` に `pub mod bridge;` を追加。

```rust
// src/bridge/mod.rs
pub mod io;
pub mod detector;
pub mod slack;
pub mod logger;

use anyhow::Result;

pub struct BridgeConfig {
    pub slack_bot_token: String,
    pub slack_app_token: String,
    pub slack_channel_id: String,
    pub notify_delay_secs: u64,
    pub session_id: String,
    pub project_name: String,
}

pub async fn run(_config: BridgeConfig, _runtime: &crate::runtime::DockerRuntime, _container_id: &str) -> Result<()> {
    todo!("bridge implementation")
}
```

各サブモジュールも空ファイルで作成（コンパイルを通すため）。

- [ ] **Step 5: ビルド確認**

Run: `cargo build 2>&1`
Expected: コンパイル成功（警告は OK）

- [ ] **Step 6: コミット**

```bash
git add -A && git commit -m "feat: add --bridge CLI options and bridge module skeleton"
```

---

## Task 2: bridge.env 読み込みとバリデーション

**Files:**
- Modify: `src/cli/run.rs` (bridge.env 読み込み、バリデーション、bridge 分岐)

- [ ] **Step 1: bridge.env 読み込みロジックを書く**

`run.rs` の `execute()` 内、セッション記録の後・Docker 起動の前に `--bridge` 判定を追加。

```rust
// bridge.env の読み込み（--bridge 指定時）
if bridge {
    let config_dir = crate::config::default_config_dir()?;
    let bridge_env_path = config_dir.join("bridge.env");

    // bridge.env または --env-file から Slack トークンを解決
    let bridge_env_file = env_file.as_deref().unwrap_or_else(|| bridge_env_path.to_str().unwrap_or(""));

    // 既存の env file 解決ロジックを流用して SLACK_BOT_TOKEN, SLACK_APP_TOKEN, SLACK_CHANNEL_ID を取得
    // ...

    // バリデーション
    // SLACK_BOT_TOKEN, SLACK_APP_TOKEN が存在すること
    // SLACK_CHANNEL_ID が --slack-channel または env file から取得できること
    // 不足時はエラーメッセージで明示して bail!()
}
```

- [ ] **Step 2: バリデーションのテスト**

bridge.env が存在しない場合のエラーメッセージ、トークン不足時のエラーメッセージを手動で確認。

Run: `cargo build && ./target/debug/vibepod run --bridge 2>&1`
Expected: bridge.env が見つからない旨のエラー

- [ ] **Step 3: コミット**

```bash
git add src/cli/run.rs && git commit -m "feat: add bridge.env loading and validation"
```

---

## Task 3: DockerRuntime に attach_container, resize_container_tty, wait_container を追加

**Files:**
- Modify: `src/runtime/docker.rs`

- [ ] **Step 1: attach_container メソッドを追加**

bollard の `AttachContainerOptions` と `AttachContainerResults` を使用。

```rust
use bollard::container::AttachContainerOptions;

pub async fn attach_container(
    &self,
    container_id: &str,
) -> Result<bollard::container::AttachContainerResults> {
    let options = AttachContainerOptions::<String> {
        stdin: Some(true),
        stdout: Some(true),
        stderr: Some(true),
        stream: Some(true),
        ..Default::default()
    };
    let results = self.docker.attach_container(container_id, Some(options)).await?;
    Ok(results)
}
```

- [ ] **Step 2: resize_container_tty メソッドを追加**

```rust
use bollard::container::ResizeContainerTtyOptions;

pub async fn resize_container_tty(
    &self,
    container_id: &str,
    width: u16,
    height: u16,
) -> Result<()> {
    let options = ResizeContainerTtyOptions {
        width,
        height,
    };
    self.docker.resize_container_tty(container_id, Some(options)).await?;
    Ok(())
}
```

- [ ] **Step 3: wait_container メソッドを追加**

```rust
use bollard::container::WaitContainerOptions;
use futures_util::StreamExt;

pub async fn wait_container(&self, container_id: &str) -> Result<i64> {
    let options = WaitContainerOptions {
        condition: "not-running",
    };
    let mut stream = self.docker.wait_container(container_id, Some(options));
    if let Some(result) = stream.next().await {
        let response = result?;
        Ok(response.status_code)
    } else {
        Ok(0)
    }
}
```

- [ ] **Step 4: ビルド確認**

Run: `cargo build 2>&1`
Expected: コンパイル成功

- [ ] **Step 5: コミット**

```bash
git add src/runtime/docker.rs && git commit -m "feat: add attach_container, resize_container_tty, wait_container to DockerRuntime"
```

---

## Task 4: bridge::logger — JSONL ログ記録

**Files:**
- Create: `src/bridge/logger.rs`
- Create: `tests/bridge_logger_test.rs`

- [ ] **Step 1: テストを書く**

```rust
// tests/bridge_logger_test.rs
use vibepod::bridge::logger::BridgeLogger;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_log_notified_event() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("test-session.jsonl");
    let mut logger = BridgeLogger::new(&log_path).unwrap();

    logger.log_notified("Do you want to proceed? (y/n)").unwrap();

    let content = fs::read_to_string(&log_path).unwrap();
    let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(line["event"], "notified");
    assert_eq!(line["last_lines"], "Do you want to proceed? (y/n)");
    assert!(line["ts"].is_string());
}

#[test]
fn test_log_responded_event() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("test-session.jsonl");
    let mut logger = BridgeLogger::new(&log_path).unwrap();

    logger.log_responded("slack_button", "y\n", 35).unwrap();

    let content = fs::read_to_string(&log_path).unwrap();
    let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(line["event"], "responded");
    assert_eq!(line["source"], "slack_button");
    assert_eq!(line["stdin_sent"], "y\n");
    assert_eq!(line["response_time_seconds"], 35);
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test bridge_logger 2>&1`
Expected: コンパイルエラー（BridgeLogger が未定義）

- [ ] **Step 3: BridgeLogger を実装**

```rust
// src/bridge/logger.rs
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
        Ok(Self { path: path.to_path_buf() })
    }

    pub fn log_notified(&mut self, last_lines: &str) -> Result<()> {
        let event = NotifiedEvent {
            ts: Local::now().to_rfc3339(),
            event: "notified",
            last_lines,
        };
        self.append(&serde_json::to_string(&event)?)
    }

    pub fn log_responded(&mut self, source: &str, stdin_sent: &str, response_time_seconds: u64) -> Result<()> {
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
```

- [ ] **Step 4: テスト実行**

Run: `cargo test bridge_logger 2>&1`
Expected: 全テスト PASS

- [ ] **Step 5: コミット**

```bash
git add src/bridge/logger.rs tests/bridge_logger_test.rs Cargo.toml && git commit -m "feat: implement bridge logger (JSONL event recording)"
```

---

## Task 5: bridge::detector — 無音検知と状態遷移

**Files:**
- Create: `src/bridge/detector.rs`
- Create: `tests/bridge_detector_test.rs`

- [ ] **Step 1: テストを書く（状態遷移）**

```rust
// tests/bridge_detector_test.rs
use vibepod::bridge::detector::{IdleDetector, DetectorState, DetectorEvent};
use std::time::Duration;

#[tokio::test]
async fn test_initial_state_is_buffering() {
    let detector = IdleDetector::new(Duration::from_secs(5));
    assert!(matches!(detector.state(), DetectorState::Buffering));
}

#[tokio::test]
async fn test_output_resets_timer() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"hello world\n");
    assert!(matches!(detector.state(), DetectorState::Buffering));
}

#[tokio::test]
async fn test_terminal_input_clears_buffer() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"some output\n");
    detector.on_terminal_input();
    assert!(detector.buffer_content().is_empty());
}

#[test]
fn test_buffer_truncation_by_lines() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    // 50行の出力を追加（上限40行）
    for i in 0..50 {
        detector.on_output(format!("line {}\n", i).as_bytes());
    }
    let content = detector.buffer_for_slack();
    let lines: Vec<&str> = content.lines().collect();
    // 先頭に truncated メッセージ + 40行
    assert!(lines[0].contains("truncated"));
    assert_eq!(lines.len(), 41);
}

#[test]
fn test_buffer_truncation_by_chars() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    // 2500文字を超える長い行を追加
    let long_line = "x".repeat(3000) + "\n";
    detector.on_output(long_line.as_bytes());
    let content = detector.buffer_for_slack();
    assert!(content.len() <= 2500 + 50); // truncation message margin
}

#[test]
fn test_ansi_stripped_in_slack_buffer() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"\x1b[31mred text\x1b[0m\n");
    let content = detector.buffer_for_slack();
    assert!(!content.contains("\x1b["));
    assert!(content.contains("red text"));
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test bridge_detector 2>&1`
Expected: コンパイルエラー

- [ ] **Step 3: IdleDetector を実装**

```rust
// src/bridge/detector.rs
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum DetectorState {
    Buffering,
    Idle,
    WaitingResponse,
}

pub enum DetectorEvent {
    Notify(String),  // バッファ内容（Slack 送信用）
    ResponseReceived,
}

pub struct IdleDetector {
    state: DetectorState,
    buffer: Vec<u8>,
    delay: Duration,
    last_output_at: Option<Instant>,
    terminal_input_since_last_output: bool,
}

impl IdleDetector {
    pub fn new(delay: Duration) -> Self { /* ... */ }
    pub fn state(&self) -> &DetectorState { /* ... */ }
    pub fn on_output(&mut self, data: &[u8]) { /* バッファ追加、タイマーリセット */ }
    pub fn on_terminal_input(&mut self) { /* Buffering: バッファクリア+タイマーリセット */ }
    pub fn check_idle(&mut self) -> Option<DetectorEvent> { /* N秒経過チェック → Notify イベント */ }
    pub fn on_response(&mut self) { /* WaitingResponse → Buffering、バッファクリア */ }
    pub fn on_output_resumed(&mut self) { /* WaitingResponse 中に出力再開 → Buffering */ }
    pub fn buffer_content(&self) -> String { /* 生バッファ */ }
    pub fn buffer_for_slack(&self) -> String { /* ANSI ストリップ + 40行/2500文字切り詰め */ }
}
```

ANSI ストリップには `strip-ansi-escapes` クレートを使用。

- [ ] **Step 4: テスト実行**

Run: `cargo test bridge_detector 2>&1`
Expected: 全テスト PASS

- [ ] **Step 5: コミット**

```bash
git add src/bridge/detector.rs tests/bridge_detector_test.rs && git commit -m "feat: implement idle detector with buffer management"
```

---

## Task 6: bridge::io — bollard attach とターミナル raw mode

**Files:**
- Create: `src/bridge/io.rs`

- [ ] **Step 1: TerminalGuard（RAII）を実装**

```rust
// src/bridge/io.rs
use std::os::unix::io::AsRawFd;
use anyhow::Result;

/// ターミナルを raw mode に設定し、Drop で自動復元する RAII ガード。
/// panic hook も設定して、パニック時にもターミナルを復元する。
pub struct TerminalGuard {
    original_termios: libc::termios,
    fd: i32,
}

impl TerminalGuard {
    pub fn new() -> Result<Self> {
        let fd = std::io::stdin().as_raw_fd();
        let mut termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
            anyhow::bail!("Failed to get terminal attributes");
        }
        let original = termios;

        // raw mode 設定
        unsafe { libc::cfmakeraw(&mut termios) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
            anyhow::bail!("Failed to set terminal to raw mode");
        }

        // panic hook でターミナル復元
        let restore_termios = original;
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &restore_termios) };
            old_hook(info);
        }));

        Ok(Self { original_termios: original, fd })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original_termios) };
    }
}
```

- [ ] **Step 2: bridge I/O ループを実装**

コンテナの stdout → ターミナル stdout 転送、ターミナル stdin → コンテナ stdin 転送、SIGWINCH ハンドリングを行う非同期ループ。

```rust
use tokio::sync::mpsc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bollard::container::AttachContainerResults;

pub struct BridgeIo {
    output_tx: mpsc::Sender<Vec<u8>>,  // detector へ出力を送信
    input_rx: mpsc::Receiver<Vec<u8>>, // slack/terminal から入力を受信
}

impl BridgeIo {
    /// メインの I/O ループ。コンテナの attach ストリームとターミナルの間をブリッジする。
    pub async fn run(
        attach_result: AttachContainerResults,
        output_tx: mpsc::Sender<Vec<u8>>,
        mut stdin_rx: mpsc::Receiver<Vec<u8>>,
        runtime: &crate::runtime::DockerRuntime,
        container_id: &str,
    ) -> Result<()> {
        // 実装: tokio::select! で
        // 1. attach stdout → stdout に書き出し + output_tx に送信
        // 2. terminal stdin → attach stdin に転送
        // 3. stdin_rx (Slack応答) → attach stdin に転送
        // 4. SIGWINCH → resize_container_tty
        // 5. コンテナ終了検知
        todo!()
    }
}
```

- [ ] **Step 3: ビルド確認**

Run: `cargo build 2>&1`
Expected: コンパイル成功

- [ ] **Step 4: コミット**

```bash
git add src/bridge/io.rs && git commit -m "feat: implement bridge I/O with terminal raw mode and attach streaming"
```

---

## Task 7: bridge::slack — Slack Socket Mode 接続と通知

**Files:**
- Create: `src/bridge/slack.rs`
- Create: `tests/bridge_slack_test.rs`
- Modify: `Cargo.toml` (Slack 関連クレート追加)

- [ ] **Step 1: Slack クレートの選定**

`slack-morphism` のメンテナンス状況を crates.io で確認。判断基準：
- 最終リリースが6ヶ月以内か
- 現行 Slack API バージョン対応か

メンテされていなければ `reqwest` + `tokio-tungstenite` で最小サブセットを自前実装。

- [ ] **Step 2: Cargo.toml に Slack 依存追加**

選定結果に基づいて追加。自前の場合：

```toml
reqwest = { version = "0.12", features = ["json"] }
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
```

- [ ] **Step 3: SlackClient を実装**

```rust
// src/bridge/slack.rs
use anyhow::Result;
use tokio::sync::mpsc;

pub struct SlackClient {
    bot_token: String,
    app_token: String,
    channel_id: String,
    project_name: String,
    session_id: String,
}

impl SlackClient {
    pub fn new(bot_token: String, app_token: String, channel_id: String, project_name: String, session_id: String) -> Self { /* ... */ }

    /// Socket Mode WebSocket 接続を確立
    pub async fn connect(&mut self) -> Result<()> { /* ... */ }

    /// セッション開始通知を送信
    pub async fn notify_session_start(&self) -> Result<()> { /* ... */ }

    /// セッション終了通知を送信
    pub async fn notify_session_end(&self, exit_code: i64) -> Result<()> { /* ... */ }

    /// 無音検知通知を送信（Block Kit: コードブロック + Yes/No/Skip ボタン）
    /// 戻り値: メッセージの ts（更新用）
    pub async fn notify_idle(&self, buffer_content: &str) -> Result<String> { /* ... */ }

    /// 応答済みメッセージに更新（ボタン無効化）
    pub async fn update_responded(&self, message_ts: &str, response_text: &str) -> Result<()> { /* ... */ }

    /// Socket Mode イベントループ。応答を受信したら response_tx に送信。
    /// ボタン、リアクション、スレッド返信を処理。
    /// 再接続: exponential backoff（初回1秒、最大60秒、最大5回）。
    /// 5回失敗で None を返してフォールバック。
    pub async fn event_loop(&mut self, response_tx: mpsc::Sender<SlackResponse>) -> Result<()> { /* ... */ }
}

pub struct SlackResponse {
    pub text: String,        // stdin に送信するテキスト
    pub source: String,      // "slack_button", "slack_reaction", "slack_thread"
    pub message_ts: String,  // 応答済み更新用
}
```

- [ ] **Step 4: テストを書く**

```rust
// tests/bridge_slack_test.rs

#[test]
fn test_block_kit_message_structure() {
    // notify_idle で生成される Block Kit JSON が正しい構造か検証
    // section block (mrkdwn + code block) + actions block (3 buttons) を含むこと
}

#[test]
fn test_response_mapping() {
    // ボタン action_id → stdin テキストのマッピングが正しいか
    // respond_yes → "y\n", respond_no → "n\n", respond_skip → "\n"
}

#[test]
fn test_reaction_mapping() {
    // リアクション名 → stdin テキストのマッピングが正しいか
    // 👍 → "y\n", 👎 → "n\n", ⏭️ → "\n"
}
```

- [ ] **Step 5: テスト実行**

Run: `cargo test bridge_slack 2>&1`
Expected: 全テスト PASS

- [ ] **Step 6: ビルド確認**

Run: `cargo build 2>&1`
Expected: コンパイル成功

- [ ] **Step 7: コミット**

```bash
git add src/bridge/slack.rs tests/bridge_slack_test.rs Cargo.toml && git commit -m "feat: implement Slack Socket Mode client with Block Kit notifications"
```

---

## Task 8: bridge::mod — エントリポイントの統合

**Files:**
- Modify: `src/bridge/mod.rs`

- [ ] **Step 1: bridge::run() を実装**

全モジュールを統合するエントリポイント。

```rust
// src/bridge/mod.rs
pub async fn run(
    config: BridgeConfig,
    runtime: &crate::runtime::DockerRuntime,
    container_id: &str,
) -> Result<i64> {
    // 1. ログディレクトリ作成、BridgeLogger 初期化
    // 2. SlackClient 初期化・接続
    // 3. セッション開始通知
    // 4. TerminalGuard で raw mode 設定
    // 5. attach_container でストリーム取得
    // 6. mpsc channel 作成:
    //    - output_tx/rx: io → detector（コンテナ出力）
    //    - slack_response_tx/rx: slack → メインループ（Slack 応答）
    //    - stdin_tx/rx: メインループ → io（stdin 注入）
    // 7. tokio::spawn で各タスクを起動:
    //    - BridgeIo::run（I/O 転送）
    //    - SlackClient::event_loop（Slack イベント受信）
    //    - detector ループ（無音検知 + 通知トリガー）
    // 8. メインループ: detector イベントと Slack 応答を処理
    //    - DetectorEvent::Notify → slack.notify_idle() + logger.log_notified()
    //    - SlackResponse → stdin_tx に送信 + slack.update_responded() + logger.log_responded()
    //    - ターミナル入力検知 → AtomicBool ガードで Slack 応答をブロック
    // 9. コンテナ終了検知 → wait_container で exit code 取得
    // 10. セッション終了通知
    // 11. TerminalGuard drop でターミナル復元
    // 12. exit code を返却
    todo!()
}
```

- [ ] **Step 2: ビルド確認**

Run: `cargo build 2>&1`
Expected: コンパイル成功

- [ ] **Step 3: コミット**

```bash
git add src/bridge/mod.rs && git commit -m "feat: integrate bridge modules in entry point"
```

---

## Task 9: run.rs に bridge 分岐を統合

**Files:**
- Modify: `src/cli/run.rs`

- [ ] **Step 1: ContainerConfig 構築をヘルパー関数に抽出**

fire-and-forget モードの ContainerConfig 構築ロジック（既存 run.rs 336-355行目）をヘルパー関数 `build_container_config()` に抽出し、bridge モードと fire-and-forget モードで共有する。

- [ ] **Step 2: bridge 分岐ロジックを追加**

`execute()` 内、セッション記録後、既存の `if interactive` / `else` 分岐の**前**に `if bridge` 判定を配置し、early return する。

```rust
// セッション記録後（既存コードの line 68 以降）
// 環境変数解決、認証トークン取得等の共通処理の後

if bridge {
    // bridge.env / --env-file から Slack トークン取得（Task 2 で実装済み）
    // BridgeConfig 構築
    // ContainerConfig 構築（共有ヘルパー使用）
    let container_config = build_container_config(/* ... */);
    let container_id = runtime.create_and_start_container(&container_config).await?;

    // bridge::run() に委譲
    let exit_code = crate::bridge::run(bridge_config, &runtime, &container_id).await?;

    // クリーンアップ
    runtime.stop_container(&container_id, 10).await?;
    runtime.remove_container(&container_id).await?;

    // temp .claude.json 削除
    // exit code に応じた終了処理

    return Ok(());
}

// 以降は既存の interactive / fire-and-forget パス（変更なし）
```

- [ ] **Step 2: ビルド確認**

Run: `cargo build 2>&1`
Expected: コンパイル成功

- [ ] **Step 3: コミット**

```bash
git add src/cli/run.rs && git commit -m "feat: integrate bridge mode into run command"
```

---

## Task 10: 手動統合テスト

**Files:** なし（手動テスト）

- [ ] **Step 1: Slack App 作成**

Slack App を作成し、必要なスコープ・Socket Mode・Interactivity を設定。`bridge.env` を作成。

```bash
# {vibepod_config_dir}/bridge.env
SLACK_BOT_TOKEN="xoxb-..."
SLACK_APP_TOKEN="xapp-..."
SLACK_CHANNEL_ID=C...
```

- [ ] **Step 2: bridge なしの動作確認**

Run: `vibepod run`
Expected: 既存の動作と同一（regression なし）

- [ ] **Step 3: bridge ありの基本動作確認**

Run: `vibepod run --bridge --notify-delay 5`
Expected:
- ターミナルに Claude Code の出力が表示される（透過）
- 5秒無音で Slack に通知が届く
- Slack のボタンをクリックすると stdin に入力される
- セッション終了時に Slack に完了通知

- [ ] **Step 4: 各応答手段のテスト**

1. Slack ボタン（Yes/No/Skip）→ 対応する入力が stdin に送信される
2. リアクション（👍/👎/⏭️）→ 同上
3. スレッド返信 → テキストが stdin に送信される
4. ターミナルから直接入力 → Slack 通知がキャンセルされる

- [ ] **Step 5: ログ確認**

`{vibepod_config_dir}/bridge-logs/` にセッションの JSONL ログが記録されていることを確認。

- [ ] **Step 6: --prompt モードでの確認**

Run: `vibepod run --bridge --prompt "hello" --notify-delay 5`
Expected: fire-and-forget モードでも bridge 通知が動作する

- [ ] **Step 7: コミット（最終調整があれば）**

```bash
git add -A && git commit -m "fix: adjustments from manual integration testing"
```

---

## Task Summary

| Task | Description | Estimated Complexity |
|------|------------|---------------------|
| 1 | CLI オプション + bridge スケルトン | Low |
| 2 | bridge.env 読み込み + バリデーション | Low |
| 3 | DockerRuntime に attach/resize 追加 | Low |
| 4 | bridge::logger (JSONL) | Low |
| 5 | bridge::detector (無音検知) | Medium |
| 6 | bridge::io (pty I/O) | High |
| 7 | bridge::slack (Socket Mode) | High |
| 8 | bridge::mod (統合) | Medium |
| 9 | run.rs に bridge 分岐統合 | Medium |
| 10 | 手動統合テスト | Medium |

**依存関係:**
```
Task 1 (CLI + skeleton)
  ├── Task 2 (bridge.env)
  ├── Task 3 (DockerRuntime 拡張)
  ├── Task 4 (logger)
  └── Task 5 (detector)

Task 6 (io) ← Task 1, 3
Task 7 (slack) ← Task 1

Task 8 (統合) ← Task 4, 5, 6, 7
Task 9 (run.rs 分岐) ← Task 2, 8
Task 10 (手動テスト) ← Task 9
```

Task 1 完了後に Task 2-5 は並行実行可能。Task 6-7 も並行可能。
