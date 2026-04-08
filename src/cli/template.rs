//! `vibepod template` subcommand implementation.
//!
//! `vibepod template list` / `vibepod template set-default <name>` を提供する。
//! 実 mount 処理は `src/cli/run/template.rs` 側、こちらは UI と管理操作のみ。

use anyhow::{bail, Result};
use std::path::Path;

use crate::cli::run::template::{embedded_template_names, user_template_names};
use crate::config;

/// グローバル `~/.config/vibepod/config.toml` から `[run] default_prompt_template`
/// の値を直接読む（プロジェクト設定の override は適用しない）。
///
/// `template list` の `<<default>>` 表示と `template set-default` が更新する
/// 書き込み先を一致させるための helper。
fn read_global_default_prompt_template(global_config_dir: &Path) -> Option<String> {
    let config_path = global_config_dir.join("config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let parsed: toml::Value = toml::from_str(&content).ok()?;
    parsed
        .get("run")
        .and_then(|run| run.get("default_prompt_template"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// `vibepod template list`: 公式 + ユーザー追加の template 一覧を表示。
///
/// 出力形式:
/// ```text
/// rust-code(embedded) <<default>>
/// review(embedded)
/// my-custom
/// ```
///
/// ユーザー追加 template に embedded と同名のものがあれば、それは
/// 「ユーザー override」として embedded 側を非表示にする。
pub fn list() -> Result<()> {
    let config_dir = config::default_config_dir()?;

    // 初回呼び出しで embed を展開（既存があれば skip）
    crate::cli::run::template::extract_embedded_templates_if_missing(&config_dir)?;

    let user_names = user_template_names(&config_dir);
    let embedded_names: Vec<String> = embedded_template_names()
        .into_iter()
        // ユーザー追加側に同名があれば embedded は除外（override）
        .filter(|n| !user_names.contains(n))
        .collect();

    // デフォルト template 名は **global config.toml のみ** から読む。
    // VibepodConfig::load() を通すとプロジェクト `.vibepod/config.toml`
    // の override が効いてしまい、`template set-default` がグローバル
    // 設定を更新しても `template list` の表示が変わらない不整合が起きる
    // （`set-default` は global 限定の操作なので、`list` の `<<default>>`
    // 表示もグローバル値に揃えるのが一貫性のある挙動）。
    let default_name = read_global_default_prompt_template(&config_dir);

    let mut all: Vec<(String, bool, bool)> = Vec::new(); // (name, is_embedded, is_default)
    for name in &embedded_names {
        let is_default = default_name.as_deref() == Some(name.as_str());
        all.push((name.clone(), true, is_default));
    }
    for name in &user_names {
        let is_default = default_name.as_deref() == Some(name.as_str());
        all.push((name.clone(), false, is_default));
    }

    // 表示順: embedded 先、その中で alphabetical、user も alphabetical
    // 既に each sub-list が sort 済みなので、結合したままでよい
    if all.is_empty() {
        println!("(no templates available — use `vibepod template set-default <name>` after adding templates to ~/.config/vibepod/templates/)");
        return Ok(());
    }

    for (name, is_embedded, is_default) in &all {
        let mut line = name.clone();
        if *is_embedded {
            line.push_str("(embedded)");
        }
        if *is_default {
            line.push_str(" <<default>>");
        }
        println!("{}", line);
    }

    Ok(())
}

/// `vibepod template set-default <name>`: デフォルト template を設定。
///
/// 指定された template が list に存在することを検証し、存在すれば
/// `~/.config/vibepod/config.toml` の `[run] default_prompt_template`
/// を更新する。存在しない場合はエラー（available list 提示付き）。
pub fn set_default(name: &str) -> Result<()> {
    let config_dir = config::default_config_dir()?;
    crate::cli::run::template::extract_embedded_templates_if_missing(&config_dir)?;

    let embedded = embedded_template_names();
    let user = user_template_names(&config_dir);

    let exists = embedded.iter().any(|n| n == name) || user.iter().any(|n| n == name);
    if !exists {
        // 利用可能な一覧をエラーメッセージに含める
        let mut available: Vec<String> = embedded;
        for n in &user {
            if !available.contains(n) {
                available.push(n.clone());
            }
        }
        available.sort();
        let available_str = if available.is_empty() {
            "(none)".to_string()
        } else {
            available.join(", ")
        };
        bail!(
            "Template '{}' not found. Available templates: {}",
            name,
            available_str
        );
    }

    config::set_default_prompt_template(&config_dir, Some(name))?;
    println!("Default template set to: {}", name);
    Ok(())
}
