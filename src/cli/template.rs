//! `vibepod template` subcommand implementation.
//!
//! `vibepod template list` / `vibepod template set-default <name>` を提供する。
//! 実 mount 処理は `src/cli/run/template.rs` 側、こちらは UI と管理操作のみ。

use anyhow::{bail, Result};

use crate::cli::run::template::{embedded_template_names, user_template_names};
use crate::config;

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

    // デフォルト template 名を config.toml から読む
    let vibepod_config =
        config::VibepodConfig::load(&std::env::current_dir()?, &config_dir).unwrap_or_default();
    let default_name = vibepod_config.default_prompt_template();

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
