---
name: rust-error-discipline
description: Rust の error handling 規律を強制する。`unwrap()` / `expect()` 禁止、`?` で伝播、`anyhow::Context` で文脈付与。use when Rust コードを書く / 修正する / review するとき
---

# Rust Error Discipline

Rust で error を扱うときは以下を厳守する。例外は明示的に理由を書いた場合のみ。

## Rule 1: `unwrap()` / `expect()` は原則禁止

プロダクションコード内での `unwrap()` / `expect()` は panic を誘発する **障害源**。使っていいのは以下の場合のみ:

- **テストコード** (`#[cfg(test)]` 配下、`tests/` ディレクトリ内)
- **panic しないことが論理的に自明** な場合 (例: regex の compile で pattern が literal)

後者の場合、**必ず理由をコメントで明記する**:

```rust
// PANIC: このパターンは literal で正しいことがコンパイル時に検証可能なため、
// コンパイル時にしか失敗しない。実行時 panic の可能性なし。
let re = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
```

## Rule 2: `?` で伝播する

呼び出し元に error を返す場合は `?` を使う。`match` で hand-unwrap しない:

```rust
// Bad
let content = match std::fs::read_to_string(path) {
    Ok(s) => s,
    Err(e) => return Err(e.into()),
};

// Good
let content = std::fs::read_to_string(path)?;
```

## Rule 3: `anyhow::Context` で文脈を付与する

error が上位に伝わる途中で「何をしていたときのエラーか」が失われないように `.with_context(|| ...)` を使う:

```rust
use anyhow::Context;

let content = std::fs::read_to_string(&path)
    .with_context(|| format!("Failed to read config file: {}", path.display()))?;
```

- **`context()` より `with_context()` を優先**: 後者は closure で遅延評価なので、hot path で無駄な allocation を避けられる。
- **context には「何を」「どこで」を含める**: ファイルパス・引数値・context 情報を具体的に。

## Rule 4: application vs library

- **application コード** (bin crate / main 付近): `anyhow::Result<T>` を使う。error は `anyhow::Error` に統一して `?` で上まで伝播。
- **library コード** (lib crate): 独自 error type を `thiserror` で定義する。呼び出し側が match できるように variant を分ける:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid TOML syntax in {path}: {source}")]
    ParseError {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}
```

## Rule 5: panic の理由は明記する

本当に panic が必要な場合（内部不変条件の違反で、プログラムを続行できない）:

```rust
// PANIC: BUG if this vector is empty here. Caller guarantees non-empty input
// via `validate_non_empty` at <file>:<line>.
let first = items.first().expect("items must be non-empty (invariant)");
```

**プロダクションパスでの無言の panic は禁止**。

## Rule 6: error 情報を捨てない

`Result::ok()` や `.ok()?` で error を潰すと、障害時のデバッグが不可能になる。デフォルトは伝播、意図的に捨てる場合はコメントで理由を書く:

```rust
// INFO: ここで read_to_string が失敗するのは「ファイルが存在しない」
// ケース（未設定状態）であり、それは正常な挙動なので None として扱う。
let content = std::fs::read_to_string(&path).ok()?;
```

## Self-check リスト

実装 / review 時に以下を確認:

- [ ] `unwrap()` / `expect()` は全てテストコード、またはコメント付きの論理的自明ケースのみか
- [ ] error は `?` で伝播しているか
- [ ] 上位に伝わる前に `.with_context(...)` で文脈が付いているか
- [ ] `.ok()` / `.ok()?` / `let _ = ...` で潰した error に理由コメントがあるか
- [ ] library 境界では独自 error type で variant が分かれているか
