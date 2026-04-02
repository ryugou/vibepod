# --review の進化（レビュワー選択 + Codex 統合 + Copilot re-review）

## 概要

`--review` オプションを拡張し、複数のレビュワー（Copilot, Codex）を設定ベースで選択・実行できるようにする。

## CLI の変更

### `--review` オプションの拡張

現在: `--review` (bool)
変更後: `--review` (Optional<String>)

```rust
/// Auto-review after implementation. Uses config reviewers if no value specified.
/// Possible values: copilot, codex
#[arg(long, num_args = 0..=1, default_missing_value = "")]
review: Option<String>,
```

- `--review` → config の `reviewers` を使用
- `--review copilot` → Copilot のみ
- `--review codex` → Codex のみ
- `--review` なし → レビューしない

## レビュワー解決ロジック

`src/cli/run.rs` に `resolve_reviewers()` を追加:

```rust
fn resolve_reviewers(review_arg: &Option<String>, config: &VibepodConfig) -> Vec<String> {
    match review_arg {
        None => vec![],  // --review なし
        Some(explicit) if !explicit.is_empty() => vec![explicit.clone()],  // --review copilot
        Some(_) => config.reviewers(),  // --review（値なし）→ config から
    }
}
```

## レビュワー別プロンプト生成

`build_review_prompt()` を `build_review_prompt(prompt, reviewers)` に変更。

### Copilot レビュープロンプト（既存を改良）

```
実装が完了したら、以下の Copilot レビューフローを実行すること:
1. 現在のブランチが main の場合は、新しいフィーチャーブランチを作成する
2. 変更内容をコミットする（Conventional Commits 準拠）
3. git push -u origin <ブランチ名> でリモートに push する
4. gh pr create で PR を作成する（ベースブランチは main）
5. gh pr edit <PR番号> --add-reviewer copilot で Copilot レビューを依頼する
6. 30 秒間隔で最大 10 回 gh api repos/{owner}/{repo}/pulls/{number}/reviews を実行して確認する
7. レビューコメントがあれば修正する
8. 修正をコミットして git push で PR を更新する
9. gh api repos/{owner}/{repo}/pulls/{number}/requested_reviewers --method POST -f "reviewers[]=copilot" で re-review を依頼する
10. 再度 30 秒間隔で最大 5 回レビュー結果を確認する
11. 最終的な PR の URL を出力する
```

### Codex レビュープロンプト（新規）

```
実装が完了したら、以下の Codex レビューフローを実行すること:
1. 現在のブランチが main の場合は、新しいフィーチャーブランチを作成する
2. 変更内容をコミットする（Conventional Commits 準拠）
3. codex review を実行する
4. レビューで指摘された問題があれば修正する
5. 修正したら再度 codex review を実行する
6. 指摘がなくなるまでステップ 4-5 を最大 3 回繰り返す
7. git push -u origin <ブランチ名> でリモートに push する
8. gh pr create で PR を作成する（ベースブランチは main）
9. 最終的な PR の URL を出力する
```

### 両方指定時（reviewers = ["copilot", "codex"]）

Codex → Copilot の順で実行（ローカルレビューを先に通してから PR レビュー）:
1. Codex レビューフローを実行（ローカルで完結）
2. Copilot レビューフローを実行（PR 作成含む）

## Codex CLI の事前チェック

### `src/cli/run.rs` の `prepare_context()` 内

reviewers に "codex" が含まれている場合:

```rust
if reviewers.contains(&"codex".to_string()) {
    let codex_available = Command::new("which")
        .arg("codex")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !codex_available {
        if review_explicit {
            // --review codex で明示指定 → エラー
            bail!("Codex CLI is not installed. Install it with: npm install -g @openai/codex");
        } else {
            // config から → warning してスキップ
            eprintln!("Warning: Codex CLI not found, skipping codex review");
            reviewers.retain(|r| r != "codex");
        }
    }
}
```

### Codex 認証のコンテナ内マウント

reviewers に "codex" が含まれている場合、`~/.codex/auth.json` を read-only でマウントする。

`ContainerConfig` に `codex_auth` フィールドを追加:

```rust
pub struct ContainerConfig {
    // ... 既存フィールド
    pub codex_auth: Option<String>,  // ~/.codex/auth.json のホストパス
}
```

`build_container_config()` で:
```rust
codex_auth: if reviewers.contains(&"codex".to_string()) {
    let codex_auth = ctx.home.join(".codex/auth.json");
    if codex_auth.exists() {
        Some(codex_auth.to_string_lossy().to_string())
    } else {
        None
    }
} else {
    None
},
```

`docker.rs` のマウント生成で:
```rust
if let Some(ref codex_auth) = config.codex_auth {
    mounts.push(Mount {
        target: Some("/home/vibepod/.codex/auth.json".to_string()),
        source: Some(codex_auth.clone()),
        typ: Some(MountTypeEnum::BIND),
        read_only: Some(true),
        ..Default::default()
    });
}
```

### Codex CLI のコンテナ内インストール

reviewers に "codex" が含まれている場合、`setup_cmd` に Codex CLI のインストールを追加する。
Node.js が必要なので、`--lang node` が指定されていない場合でも Node.js をインストールする。

```rust
if reviewers.contains(&"codex".to_string()) {
    // Node.js が setup_cmd に含まれていなければ追加
    if !setup_cmd_contains_node {
        setup_parts.push("curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && apt-get install -y nodejs");
    }
    setup_parts.push("npm install -g @openai/codex");
}
```

## 変更対象ファイル

- `Cargo.toml` — バージョンを 1.5.0 に更新
- `src/cli/mod.rs` — `--review` を `Option<String>` に変更
- `src/main.rs` — 引数の受け渡し更新
- `src/cli/run.rs` — `resolve_reviewers()`, `build_review_prompt()` の拡張、Codex チェック、config 読み込み
- `src/runtime/docker.rs` — `ContainerConfig` に `codex_auth` 追加、マウント生成
- `tests/run_logic_test.rs` — `build_review_prompt` のテスト更新、`resolve_reviewers` のテスト追加
- `tests/cli_test.rs` — `--review copilot` / `--review` のパーステスト

## 検証

- `cargo check && cargo clippy && cargo fmt --check && cargo test` が通ること
- `--review` のパースが正しいこと（値なし / 値あり / なし の3パターン）
