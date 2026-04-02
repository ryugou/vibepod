# v1.4 テスト追加 + 堅牢性改善 + --review プロンプト改善

## 概要

v1.4 で追加した機能のユニットテストを追加し、堅牢性の改善と `--review` プロンプトの問題修正を行う。
外部挙動の変更は Slack channel ID バリデーションと `--review` プロンプト改善のみ。

## 1. テスト追加

### 1-1. `tests/cli_test.rs` — v1.4 CLI オプションのテスト追加

既存の cli_test.rs に以下を追加：

```rust
// --lang オプションのパース
#[test]
fn test_parse_run_with_lang() {
    let cli = Cli::parse_from(["vibepod", "run", "--lang", "rust"]);
    // lang == Some("rust") を検証
}

// --worktree オプションのパース
#[test]
fn test_parse_run_with_worktree() {
    let cli = Cli::parse_from(["vibepod", "run", "--prompt", "test", "--worktree"]);
    // worktree == true を検証
}

// --review オプションのパース
#[test]
fn test_parse_run_with_review() {
    let cli = Cli::parse_from(["vibepod", "run", "--prompt", "test", "--review"]);
    // review == true を検証
}
```

### 1-2. `tests/run_logic_test.rs` — run.rs の純粋ロジックのテスト（新規ファイル）

`detect_languages` と `get_lang_install_cmd` をテストする。
これらは現在 `run.rs` 内の private 関数なので、テストのために `pub(crate)` に変更する。

```rust
// detect_languages のテスト
#[test]
fn test_detect_rust() {
    // tempdir に Cargo.toml を作成 → detect_languages が ("rust", "Cargo.toml") を返す
}

#[test]
fn test_detect_node() {
    // tempdir に package.json を作成
}

#[test]
fn test_detect_multiple_languages() {
    // tempdir に Cargo.toml + package.json を作成 → 両方検出される
}

#[test]
fn test_detect_no_languages() {
    // 空の tempdir → 空の Vec
}

// get_lang_install_cmd のテスト
#[test]
fn test_lang_install_cmd_rust() {
    // "rust" → Some(...) で rustup が含まれる
}

#[test]
fn test_lang_install_cmd_unknown() {
    // "unknown" → None
}

// build_review_prompt のテスト（後述の関数抽出後）
#[test]
fn test_review_prompt_injection() {
    // prompt + review=true → レビューフロー指示が末尾に追加される
}

#[test]
fn test_no_review_prompt_unchanged() {
    // prompt + review=false → 元のプロンプトのまま
}
```

### 1-3. `tests/stream_format_test.rs` — JSONL パースのテスト（新規ファイル）

`stream_logs_formatted` のパースロジックをテストする。
パースロジック（JSONL 1行 → 表示文字列）を `docker.rs` から純粋関数として抽出する。

#### `src/runtime/docker.rs` に追加する関数

```rust
/// JSONL 1行をパースして表示用文字列に変換する。
/// 表示不要なイベント（system, allowed rate_limit）は None を返す。
/// result イベントは Err(result_text) で返す（表示せず保持するため）。
pub(crate) fn format_stream_event(line: &str) -> StreamEvent {
    // 既存の stream_logs_formatted 内のパースロジックをそのまま切り出す
}

pub(crate) enum StreamEvent {
    Display(String),       // 表示する行
    Result(String),        // result テキスト（保持用）
    Skip,                  // 表示しない
    PassThrough(String),   // パース失敗、そのまま出力
}
```

テスト:

```rust
#[test]
fn test_format_assistant_text() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ファイルを確認します"}]}}"#;
    // Display("  │  [assistant] ファイルを確認します")
}

#[test]
fn test_format_tool_use() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
    // Display("  │  [tool_use] Read { file_path: \"src/main.rs\" }")
}

#[test]
fn test_format_tool_use_truncation() {
    // input の値が 80 文字超 → "..." でトランケート
}

#[test]
fn test_format_result() {
    let line = r#"{"type":"result","result":"完了しました"}"#;
    // Result("完了しました")
}

#[test]
fn test_format_system_event_skipped() {
    let line = r#"{"type":"system","subtype":"init"}"#;
    // Skip
}

#[test]
fn test_format_rate_limit_allowed_skipped() {
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
    // Skip
}

#[test]
fn test_format_rate_limit_rejected() {
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":"2026-04-02T12:00:00Z","rateLimitType":"five_hour"}}"#;
    // Display("  │  [rate_limit] status: rejected, resets_at: 2026-04-02T12:00:00Z, type: five_hour")
}

#[test]
fn test_format_invalid_json() {
    let line = "not json at all";
    // PassThrough("not json at all")
}
```

## 2. 堅牢性改善

### 2-1. Slack channel ID のバリデーション

`src/cli/run.rs` の `run_bridge()` 内、channel ID 取得後にバリデーションを追加。

Slack channel ID のフォーマット: `C` で始まる英数字の文字列（通常 11 文字だが、長くなる場合もある）。

```rust
if !slack_channel_id.is_empty()
    && (!slack_channel_id.starts_with('C') || slack_channel_id.len() < 9)
{
    bail!(
        "Invalid Slack channel ID: '{}'. Channel IDs start with 'C' (e.g., C01ABC2DEF3).",
        slack_channel_id
    );
}
```

テスト（`tests/run_logic_test.rs` に追加）:

```rust
#[test]
fn test_valid_slack_channel_id() {
    assert!(validate_slack_channel_id("C01ABC2DEF3"));
}

#[test]
fn test_invalid_slack_channel_id_wrong_prefix() {
    assert!(!validate_slack_channel_id("U01ABC2DEF3"));
}

#[test]
fn test_invalid_slack_channel_id_too_short() {
    assert!(!validate_slack_channel_id("C123"));
}
```

バリデーションロジックはテスト可能なように `pub(crate) fn validate_slack_channel_id(id: &str) -> bool` として切り出す。

### 2-2. `--lang rust` の build-essential 追加

現在の rust インストールコマンドに `build-essential` が含まれていない。
`get_lang_install_cmd("rust")` の返り値を以下に修正:

```
apt-get update && apt-get install -y build-essential && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && . $HOME/.cargo/env
```

## 3. --review プロンプト改善

### 問題

現在の `--review` プロンプトで、Claude Code が `gh pr reviews` でレビュー結果を確認しようとして
空のコメントを PR に投稿してしまう（`ryugou` として空 body のレビューが5つ作成された）。

### 修正

`src/cli/run.rs` のレビューフロー指示を改善:

```rust
let effective_prompt = if opts.review {
    format!(
        "{}\n\n---\n\n\
        実装が完了したら、以下のレビューフローを実行すること:\n\
        1. 変更内容をコミットする（Conventional Commits 準拠）\n\
        2. `gh pr create` で PR を作成する（ベースブランチは main）\n\
        3. `gh pr edit <PR番号> --add-reviewer copilot` で GitHub Copilot のレビューを依頼する\n\
        4. Copilot のレビューが届くまで 30 秒間隔で最大 10 回 `gh api repos/{{owner}}/{{repo}}/pulls/{{number}}/reviews` を実行して確認する。\
           `gh pr review` や `gh pr comment` などの書き込み系コマンドは絶対に使わないこと（意図しないレビューコメントが作成されるため）\n\
        5. Copilot のレビューコメントがあれば内容を読み、指摘された問題を修正する\n\
        6. 修正をコミットして `git push` で PR を更新する\n\
        7. 最終的な PR の URL を出力する",
        p
    )
} else {
    p.clone()
};
```

変更点:
- ステップ 4: `gh pr reviews` → `gh api` で読み取り専用の API 呼び出しに変更
- ステップ 4: 書き込み系コマンド禁止の注意書きを追加
- ステップ 4: ポーリング間隔と回数を明記（30秒 × 最大10回）
- ステップ 2: ベースブランチを明記

## 変更対象ファイル

- `src/cli/run.rs` — `detect_languages`, `get_lang_install_cmd` を `pub(crate)` に変更。`validate_slack_channel_id` 追加。`build_review_prompt` 関数抽出。レビュープロンプト修正。rust の build-essential 追加
- `src/runtime/docker.rs` — `format_stream_event` 関数と `StreamEvent` enum を抽出
- `tests/cli_test.rs` — v1.4 CLI オプションのテスト追加
- `tests/run_logic_test.rs` — 新規。言語検出、プロンプト注入、channel ID バリデーションのテスト
- `tests/stream_format_test.rs` — 新規。JSONL パースのテスト

## 検証

- `cargo check && cargo clippy && cargo test` がすべて通ること
- 新規テストが想定通りの項目をカバーしていること
