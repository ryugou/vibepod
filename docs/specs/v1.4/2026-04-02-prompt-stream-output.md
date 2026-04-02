# --prompt モードで Claude Code の実行状況をリアルタイム表示する

## 背景

`vibepod run --prompt` で fire-and-forget 実行すると、起動画面の後、タスク完了まで一切出力がない。
Claude Code の `--output-format stream-json --verbose` を利用すれば、JSONL でイベントがリアルタイムにストリーミングされるため、これをパースして表示する。

## 仕組み

Claude Code を `-p <prompt> --output-format stream-json --verbose` で起動すると、stdout に JSONL が1行ずつ流れる。
現在の `stream_logs` はこの出力をそのまま `print!` しているが、JSONL をパースしてフォーマット表示に変更する。

## JSONL イベントと表示ルール

### 表示するイベント

| type | 条件 | 表示内容 |
|------|------|----------|
| `assistant` | `content` に `text` がある | `text` の値をそのまま表示 |
| `assistant` | `content` に `tool_use` がある | `name` と `input` の主要キーを表示 |
| `result` | 常に | `result` フィールドをそのまま表示 |
| `rate_limit_event` | `rate_limit_info.status != "allowed"` | `status`, `resetsAt`, `rateLimitType` を表示 |

### 表示しないイベント

| type | 理由 |
|------|------|
| `system` (init, hook_started, hook_response) | 内部イベント。ユーザーに不要 |
| `rate_limit_event` (`status == "allowed"`) | 正常時は不要 |

### tool_use の表示例

`input` の中身はツールによって異なる。主要キーをそのまま出す。
ただし、各値が長い場合（80文字超）はトランケートして `...` を付与する。
Edit の `old_string` / `new_string` など、コード全文が入るフィールドがそのまま出ると見づらいため。

```
  │  [tool_use] Read { file_path: "src/cli/run.rs" }
  │  [tool_use] Edit { file_path: "src/cli/run.rs", old_string: "pub struct ContainerConfig {\n    pub image: S...", new_string: "pub struct ContainerConfig {\n    pub image: S..." }
  │  [tool_use] Bash { command: "cargo check" }
  │  [tool_use] Glob { pattern: "src/**/*.rs" }
```

### text の表示例

```
  │  [assistant] ファイルを確認します。
```

### result の表示例

```
  │  [result] 実装が完了しました。変更内容: ...
```

### rate_limit_event の表示例（status != "allowed" のとき）

```
  │  [rate_limit] status: waiting, resets_at: 2026-04-02T12:00:00Z, type: five_hour
```

## 変更対象

### 1. `src/cli/run.rs` — Claude Code 起動引数の追加

L367-370 付近。`--prompt` が指定されているとき、追加の引数を渡す：

```rust
if let Some(ref p) = prompt {
    claude_args.push("-p".to_string());
    claude_args.push(p.clone());
    claude_args.push("--output-format".to_string());
    claude_args.push("stream-json".to_string());
    claude_args.push("--verbose".to_string());
}
```

### 2. `src/runtime/docker.rs` — JSONL パース・フォーマット表示

`stream_logs` に加えて、JSONL をパースしてフォーマット表示する `stream_logs_formatted` を追加する。

- 1行ずつ `serde_json::Value` でパース
- `type` フィールドで分岐して表示ルールに従い出力
- パースに失敗した行はそのまま出力（フォールバック）

### 3. `src/cli/run.rs` — fire-and-forget パスでの呼び出し変更

L636-637 付近。`--prompt` が指定されている場合は `stream_logs_formatted` を呼ぶ：

```rust
tokio::select! {
    _ = if prompt.is_some() {
        runtime.stream_logs_formatted(&container_id)
    } else {
        runtime.stream_logs(&container_id)
    } => {
        // Agent finished naturally
    }
    // ...
}
```

## 影響範囲

- `--prompt` モードのみ。interactive / resume / bridge モードは変更なし
- `stream_logs`（既存）はそのまま残す
