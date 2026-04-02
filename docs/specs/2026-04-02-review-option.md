# --review オプション（Copilot レビューフロー自動注入）

## 背景

`--prompt` で fire-and-forget 実行した後、実装結果のレビューが行われないまま完了する。
`--review` オプションを追加し、プロンプト末尾にレビューフローの指示を自動注入することで、
PR 作成 → Copilot レビュー → 指摘修正 → PR 更新 までを自動で行えるようにする。

## CLI 引数

### `src/cli/mod.rs`

```rust
/// Auto-create PR and request GitHub Copilot review after implementation (requires --prompt)
#[arg(long)]
review: bool,
```

## バリデーション

### `src/cli/run.rs`

`--review` は `--prompt` と併用必須。`--prompt` なしで `--review` が指定された場合はエラー。

```rust
if review && prompt.is_none() {
    bail!("--review requires --prompt");
}
```

## プロンプト注入

### `src/cli/run.rs`

`--review` が指定されている場合、`prompt` の末尾にレビューフロー指示を追加する。
claude_args を組み立てる前（L367 付近）で、prompt を加工する。

```rust
let effective_prompt = if let Some(ref p) = prompt {
    if review {
        format!(
            "{}\n\n---\n\n\
            実装が完了したら、以下のレビューフローを実行すること:\n\
            1. 変更内容をコミットする（Conventional Commits 準拠）\n\
            2. `gh pr create` で PR を作成する\n\
            3. `gh pr edit <PR番号> --add-reviewer copilot` で GitHub Copilot のレビューを依頼する\n\
            4. レビュー結果を待つ: `gh pr reviews <PR番号>` でレビューコメントを確認する（数分かかる場合がある。最大3回リトライすること）\n\
            5. レビューで指摘された内容を修正する\n\
            6. 修正をコミットして PR を更新する（`git push`）\n\
            7. 最終的な PR の URL を出力する",
            p
        )
    } else {
        p.clone()
    }
} else {
    unreachable!()
};
```

その後の claude_args 組み立てで `effective_prompt` を使う:

```rust
if prompt.is_some() {
    claude_args.push("-p".to_string());
    claude_args.push(effective_prompt);
    claude_args.push("--output-format".to_string());
    claude_args.push("stream-json".to_string());
    claude_args.push("--verbose".to_string());
}
```

## main.rs への引数追加

### `src/main.rs`

`Commands::Run` の分解と `execute` 呼び出しに `review` を追加する。

## 起動時の表示

`--review` が有効な場合、起動情報に表示する:

```
Review: enabled (GitHub Copilot)
```

## 影響範囲

- `src/cli/mod.rs` — CLI 引数定義に `review` 追加
- `src/main.rs` — `review` を `execute` に渡す
- `src/cli/run.rs` — バリデーション、プロンプト注入、起動時表示、`execute` の引数追加
- コンテナ内に `gh` CLI が必要（既に Docker イメージに含まれている）
