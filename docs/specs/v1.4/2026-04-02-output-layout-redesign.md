# --prompt モードの出力レイアウト再設計

## 背景

現在の `--prompt` モードの出力は、VibePod の起動情報とClaude Code の作業ログがすべてツリー UI（`│` プレフィックス）の中に混在しており、区別しづらい。
また `[result]` がストリーミング中に表示された後、直前の `[assistant]` と内容が重複する問題がある。

## 方針

- **VibePod 自身の通知**: インデントなし（左寄せ）
- **Claude Code の作業ログ**: 区切り線の中にツリー UI（`│` プレフィックス）
- **Result**: インデントなし（左寄せ）。ストリーミング中は `[result]` を表示せず、コンテナ停止後にまとめて表示

## 出力レイアウト

```
 ██╗   ██╗██╗██████╗ ███████╗██████╗  ██████╗ ██████╗
 ██║   ██║██║██╔══██╗██╔════╝██╔══██╗██╔═══██╗██╔══██╗
 ██║   ██║██║██████╔╝█████╗  ██████╔╝██║   ██║██║  ██║
 ╚██╗ ██╔╝██║██╔══██╗██╔══╝  ██╔═══╝ ██║   ██║██║  ██║
  ╚████╔╝ ██║██████╔╝███████╗██║     ╚██████╔╝██████╔╝
   ╚═══╝  ╚═╝╚═════╝ ╚══════╝╚═╝      ╚═════╝ ╚═════╝
                                              v1.4.0

Detected git repository: vibepod
Remote: git@github.com:ryugou/vibepod.git
Branch: fix/bridge-attach-timing

Starting container...
Agent: Claude Code
Mode: fire-and-forget (--dangerously-skip-permissions)
Mount: /Users/ryugo/Developer/src/personal/vibepod → /workspace
Language: rust (detected from Cargo.toml)

Container started: vibepod-vibepod-a32dbf
Press Ctrl+C to stop the container.

────────────────────────────────────────────────────────
  │  [assistant] ファイルを確認します。
  │  [tool_use] Read { file_path: "src/cli/run.rs" }
  │  [tool_use] Edit { file_path: "src/cli/run.rs", old_string: "pub struct C...", new_string: "pub struct C..." }
  │  [tool_use] Bash { command: "cargo check" }
  │  [assistant] Both cargo check and cargo clippy pass cleanly...
────────────────────────────────────────────────────────

Result:
Both `cargo check` and `cargo clippy` pass cleanly...
1. Banner — version displayed right-aligned after ASCII art
2. Lang — --lang flag + auto-detection
3. Worktree — --worktree flag creates isolated git worktree
4. Diff — git diff summary printed after container stops

Container stopped and removed.
```

## 変更対象

### 1. `src/cli/run.rs` — 起動情報の出力からツリー UI を除去

`--prompt` モードのとき、起動情報の `println!` からツリー UI プレフィックス（`  ┌`, `  │`, `  ◇`, `  └`）を除去する。

変更前:
```rust
banner::print_banner();
println!("  ┌");
println!("  │");
println!("  ◇  Detected git repository: {}", project_name);
if let Some(ref r) = remote {
    println!("  │  Remote: {}", r);
}
println!("  │  Branch: {}", branch);
println!("  │");
// ...
println!("  ◇  Container started: {}", container_name);
println!("  │  Press Ctrl+C to stop the container.");
println!("  └\n");
```

変更後:
```rust
banner::print_banner();
println!();
println!("Detected git repository: {}", project_name);
if let Some(ref r) = remote {
    println!("Remote: {}", r);
}
println!("Branch: {}", branch);
println!();
// ...
println!("Container started: {}", container_name);
println!("Press Ctrl+C to stop the container.");
println!();
```

注意: この変更は `--prompt` モード（fire-and-forget）のみに適用する。
interactive モードは現行のツリー UI を維持する。

### 2. `src/runtime/docker.rs` の `stream_logs_formatted` — 区切り線の追加と result の分離

変更内容:
1. ストリーミング開始時に区切り線 `────────────────────────────────────────────────────────` を出力
2. `type: "result"` のとき、表示せずに結果テキストを内部に保持する
3. ストリーミング終了時に区切り線を出力
4. 返り値を `Result<Option<String>>` に変更し、保持した result テキストを返す

```rust
pub async fn stream_logs_formatted(&self, container_id: &str) -> Result<Option<String>> {
    let separator = "────────────────────────────────────────────────────────";
    println!("{}", separator);

    let mut result_text: Option<String> = None;
    // ... 既存のストリーミングループ ...
    // "result" イベント時: 表示せず result_text に保持
    // "assistant", "tool_use" 等: 既存の表示ロジック

    println!("{}", separator);
    Ok(result_text)
}
```

### 3. `src/cli/run.rs` — コンテナ停止後の Result 表示

`stream_logs_formatted` の返り値を受け取り、`Container stopped and removed.` の直前に Result を表示する。

```rust
let result_text = tokio::select! {
    result = runtime.stream_logs_formatted(&container_id) => {
        result.unwrap_or(None)
    }
    _ = tokio::signal::ctrl_c() => {
        println!("\nStopping container...");
        None
    }
};

runtime.stop_container(&container_id, 10).await.ok();
runtime.remove_container(&container_id).await.ok();

if let Some(ref text) = result_text {
    println!();
    println!("Result:");
    println!("{}", text);
}

println!();
println!("Container stopped and removed.");
```

### 4. `src/runtime/docker.rs` — tool_use 値のトランケート

既存の `stream_logs_formatted` 内の tool_use 表示で、各値が80文字を超える場合はトランケートする。

```rust
let val = v.as_str()
    .map(|s| {
        if s.len() > 80 {
            format!("\"{}...\"", &s[..77])
        } else {
            format!("\"{}\"", s)
        }
    })
    .unwrap_or_else(|| v.to_string());
```

## 影響範囲

- `--prompt` モード（fire-and-forget）の出力レイアウトのみ変更
- interactive / resume / bridge モードは変更なし（ツリー UI 維持）
- `stream_logs`（既存）はそのまま残す
