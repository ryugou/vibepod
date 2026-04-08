//! `vibepod template` subcommand implementation.
//!
//! `vibepod template list` / `vibepod template set-default <name>` を提供する。
//! 実 mount 処理は `src/cli/run/template.rs` 側、こちらは UI と管理操作のみ。

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::cli::run::template::{
    embedded_template_names, is_embedded_extracted, user_template_names,
};
use crate::config;

/// グローバル `~/.config/vibepod/config.toml` から `[run] default_prompt_template`
/// の値を直接読む（プロジェクト設定の override は適用しない）。
///
/// `template list` の `<<default>>` 表示と `template set-default` が更新する
/// 書き込み先を一致させるための helper。
///
/// 戻り値:
/// - `Ok(None)`: config ファイルが存在しない、または `default_prompt_template`
///   が設定されていない (正常状態)
/// - `Ok(Some(name))`: 設定済み
/// - `Err(_)`: ファイル read 失敗 (NotFound 以外) や TOML パースエラー。
///   呼び出し側はこれを呑まずユーザーに報告する (`set-default` も同じ
///   ファイルで失敗するため、`list` 側だけ silent に成功させると
///   不整合が起きる)。
fn read_global_default_prompt_template(global_config_dir: &Path) -> Result<Option<String>> {
    let config_path = global_config_dir.join("config.toml");
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to read config file: {}", config_path.display()));
        }
    };
    let parsed: toml::Value = toml::from_str(&content).with_context(|| {
        format!(
            "Failed to parse {} as TOML: fix syntax errors first",
            config_path.display()
        )
    })?;
    // `[run]` が無ければ未設定扱い (正常)。存在するが table で無い、
    // または `default_prompt_template` が string でないなら shape エラー
    // として fail する (silent な None は `set-default` との不整合を
    // 生むので避ける — `set-default` は同じファイルで table を要求する)。
    let run = match parsed.get("run") {
        None => return Ok(None),
        Some(v) => v,
    };
    let run_table = match run.as_table() {
        Some(t) => t,
        None => {
            bail!(
                "Invalid config in {}: [run] must be a TOML table",
                config_path.display()
            );
        }
    };
    match run_table.get("default_prompt_template") {
        None => Ok(None),
        Some(v) => match v.as_str() {
            Some(s) => Ok(Some(s.to_string())),
            None => bail!(
                "Invalid config in {}: [run].default_prompt_template must be a string",
                config_path.display()
            ),
        },
    }
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

    // ここでは extract を呼ばない。read-only / 権限制限された
    // `~/.config/vibepod` でも `template list` を使えるようにするため、
    // embedded template はバイナリから直接列挙し、on-disk 状況とは独立に
    // 表示する。実際の展開は `vibepod run --template <name>` の経路で
    // 必要になったときに lazy に行われる。
    //
    // 重要: 既に extract 済みの場合は `~/.config/vibepod/
    // templates/<name>/` の実ディレクトリとして存在するため、
    // `user_template_names()` の結果には embedded 名も embed されない
    // ユーザー追加名も両方含まれる。さらに、ユーザーが embedded と
    // 同名のディレクトリを自前で作って override しているケースもある。
    // 区別のため extract_template_dir は dest に `.vibepod-embedded`
    // マーカーを書く。これが存在する dir は vibepod 管理 (`(embedded)`)、
    // 存在しない dir はユーザー作成 (override or 純粋ユーザー追加)。
    let embedded_names = embedded_template_names();
    let templates_root = config_dir.join("templates");
    let user_dir_names = user_template_names(&config_dir)?;
    // 真に embed として表示する: コンパイル時 embed 集合に名前があり、
    // かつ on-disk dir にマーカーがある (= vibepod が展開した実体)。
    // マーカーが無ければ user override として扱う。
    let mut embedded_displayed: Vec<String> = Vec::new();
    let mut user_only_names: Vec<String> = Vec::new();
    for name in &user_dir_names {
        let dir = templates_root.join(name);
        if embedded_names.contains(name) && is_embedded_extracted(&dir) {
            embedded_displayed.push(name.clone());
        } else {
            user_only_names.push(name.clone());
        }
    }
    // まだ extract されていない embedded もあり得る (read-only $HOME 等で
    // extract が呼ばれない経路)。embedded 集合のうち on-disk に出ていない
    // ものは embedded として広告だけしておく。
    for name in &embedded_names {
        if !user_dir_names.contains(name) {
            embedded_displayed.push(name.clone());
        }
    }
    embedded_displayed.sort();
    embedded_displayed.dedup();

    // デフォルト template 名は **global config.toml のみ** から読む。
    // VibepodConfig::load() を通すとプロジェクト `.vibepod/config.toml`
    // の override が効いてしまい、`template set-default` がグローバル
    // 設定を更新しても `template list` の表示が変わらない不整合が起きる
    // （`set-default` は global 限定の操作なので、`list` の `<<default>>`
    // 表示もグローバル値に揃えるのが一貫性のある挙動）。
    let default_name = read_global_default_prompt_template(&config_dir)?;

    let mut all: Vec<(String, bool, bool)> = Vec::new(); // (name, is_embedded, is_default)
    for name in &embedded_displayed {
        let is_default = default_name.as_deref() == Some(name.as_str());
        all.push((name.clone(), true, is_default));
    }
    user_only_names.sort();
    for name in &user_only_names {
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
    // extract は呼ばない。embedded template はバイナリから直接判定でき、
    // user template は on-disk から列挙できるため、`set-default` の検証
    // 自体は read-only `~/.config/vibepod` でも (config.toml への書き込み
    // 権限さえあれば) 動く。実際の展開は `run --template <name>` 経路で
    // 必要になった時点で lazy に行う。
    let embedded = embedded_template_names();
    let user = user_template_names(&config_dir)?;

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
