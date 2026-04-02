# vibepod.toml — プロジェクト設定ファイル

## 概要

vibepod の設定を TOML ファイルで管理する。2層構造（グローバル + プロジェクト）。

## 設定ファイルの場所と優先順位

1. **プロジェクト設定**: `.vibepod/config.toml`（プロジェクトルートの `.vibepod/` 内。`.gitignore` 対象のまま）
2. **グローバル設定**: `~/.config/vibepod/config.toml`

プロジェクト設定にキーがあればそちらを優先。なければグローバルにフォールバック。どちらにもなければデフォルト値。

## TOML スキーマ

```toml
[review]
# 使用するレビュワーのリスト。--review 時にこの順番で実行される
# 有効な値: "copilot", "codex"
reviewers = ["copilot"]

[run]
# デフォルトの言語ツールチェイン。未指定なら自動検出
lang = "rust"
```

## 変更対象

### 1. `Cargo.toml` — toml クレートの追加

```toml
toml = "0.8"
```

### 2. `src/config/mod.rs` — vibepod_config モジュールの追加

```rust
mod global;
mod projects;
mod vibepod_config;

pub use global::*;
pub use projects::*;
pub use vibepod_config::*;
```

### 3. `src/config/vibepod_config.rs` — 新規ファイル

```rust
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VibepodConfig {
    pub review: Option<ReviewConfig>,
    pub run: Option<RunConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReviewConfig {
    pub reviewers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RunConfig {
    pub lang: Option<String>,
}

impl VibepodConfig {
    /// プロジェクト設定 → グローバル設定の順でマージした設定を返す
    pub fn load(project_dir: &Path, global_config_dir: &Path) -> Result<Self> {
        let project_config = Self::load_file(&project_dir.join(".vibepod/config.toml"));
        let global_config = Self::load_file(&global_config_dir.join("config.toml"));

        Ok(Self::merge(project_config, global_config))
    }

    fn load_file(path: &Path) -> Option<VibepodConfig> {
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    }

    fn merge(project: Option<Self>, global: Option<Self>) -> Self {
        match (project, global) {
            (Some(p), Some(g)) => {
                // プロジェクト側にキーがあればそちら、なければグローバル
                VibepodConfig {
                    review: p.review.or(g.review),
                    run: p.run.or(g.run),
                }
            }
            (Some(p), None) => p,
            (None, Some(g)) => g,
            (None, None) => Self::default(),
        }
    }

    pub fn reviewers(&self) -> Vec<String> {
        self.review
            .as_ref()
            .and_then(|r| r.reviewers.clone())
            .unwrap_or_default()
    }

    pub fn lang(&self) -> Option<String> {
        self.run.as_ref().and_then(|r| r.lang.clone())
    }
}
```

### 4. `src/cli/run.rs` — config の読み込みと適用

`prepare_context()` 内で `VibepodConfig::load()` を呼び、以下に適用する:

- `lang`: config の `lang` を `--lang` のデフォルト値として使用。`--lang` CLI オプションが明示指定されていればそちらを優先。どちらもなければ自動検出
- `reviewers`: `build_review_prompt()` に渡す（次の spec で使用）

### 5. テスト — `tests/config_test.rs` に追加

```rust
#[test]
fn test_load_vibepod_config_project_only() { ... }

#[test]
fn test_load_vibepod_config_global_only() { ... }

#[test]
fn test_load_vibepod_config_merge_priority() { ... }

#[test]
fn test_load_vibepod_config_none() { ... }

#[test]
fn test_reviewers_default_empty() { ... }
```

## 検証

- `cargo check && cargo clippy && cargo fmt --check && cargo test` が通ること
