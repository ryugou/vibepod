//! `vibepod template` subcommand implementation.
//!
//! `vibepod template list` / `vibepod template set-default <name>` を提供する。
//! 実 mount 処理は `src/cli/run/template.rs` 側、こちらは UI と管理操作のみ。

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::cli::run::template::{
    embedded_template_names, is_embedded_extracted, user_template_names,
};
use crate::config;
use crate::ui::sanitize::sanitize_single_line;

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
/// rust/impl(embedded) <<default>>
/// generic/review(embedded)
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

/// `vibepod template reset <name> [--force]`: 埋め込み template を
/// 強制的に再展開する。既存の `~/.config/vibepod/templates/<name>/`
/// は削除され、vibepod binary から新しいコピーが展開される。
///
/// 用途: vibepod 本体をアップグレードして embedded template の中身
/// (特に `plugins/`) が更新されたとき、既に extract 済みのユーザーが
/// 新しい bundle を取り込むために使う。通常の extract は冪等で既存を
/// 保護するため、このコマンドが無いと古い bundle を持ち続ける。
///
/// **警告**: 対象 dir 配下でユーザーが行った編集は消える。`--force`
/// を明示的に付けないと拒否する。
pub fn reset(name: &str, force: bool) -> Result<()> {
    let config_dir = config::default_config_dir()?;
    reset_in(&config_dir, name, force)
}

/// `reset()` の内部実装。`config_dir` を明示的に受け取るため、HOME 環境
/// 変数を汚さずに testable。本体は `reset()` からのみ呼ばれる。
pub(crate) fn reset_in(config_dir: &Path, name: &str, force: bool) -> Result<()> {
    // embedded 集合にある名前だけ reset 対象にする。user-only template の
    // reset は意味不明 (復元元が無い) なのでエラーにする。
    let embedded = crate::cli::run::template::embedded_template_names();
    if !embedded.iter().any(|n| n == name) {
        bail!(
            "Template '{}' is not an embedded template and cannot be reset. \
             Available embedded templates: {}",
            name,
            if embedded.is_empty() {
                "(none)".to_string()
            } else {
                embedded.join(", ")
            }
        );
    }

    let target: PathBuf = config_dir.join("templates").join(name);

    // 既存 dir が **user override** (marker 無し) の場合は reset を拒否する。
    // embedded と同名だからといってユーザーが手作業で作った template を
    // 勝手に置き換えてよいわけではない。`template list` も override を
    // 別扱いで表示するので、reset の挙動もそれに揃える。
    if target.exists() && !crate::cli::run::template::is_embedded_extracted(&target) {
        bail!(
            "Refusing to reset template '{}': the on-disk directory '{}' is a user \
             override (no .vibepod-embedded marker), not a vibepod-managed extraction. \
             Remove or rename the directory manually if you want to replace it with \
             the embedded copy.",
            name,
            target.display()
        );
    }

    if !force {
        bail!(
            "Refusing to reset template '{}' without --force. The target directory \
             '{}' will be removed and re-extracted from the vibepod binary. Any user \
             edits inside that directory will be lost. Pass --force to confirm.",
            name,
            target.display()
        );
    }

    // 既存 entry の削除 (存在する場合のみ)。存在しなくてもエラーにしない
    // (単に「最新バイナリから fresh 展開する」になる)。
    //
    // symlink や regular file の可能性もある: `symlink_metadata()` で
    // follow せず実体を見て、それぞれ適切な remove 関数を呼ぶ。
    // (`remove_dir_all` は symlink 相手だと `Not a directory` で失敗する)
    match std::fs::symlink_metadata(&target) {
        Ok(meta) => {
            let ft = meta.file_type();
            if ft.is_symlink() || ft.is_file() {
                std::fs::remove_file(&target).with_context(|| {
                    format!(
                        "Failed to remove existing template entry (symlink or file): {}",
                        target.display()
                    )
                })?;
            } else if ft.is_dir() {
                std::fs::remove_dir_all(&target).with_context(|| {
                    format!(
                        "Failed to remove existing template directory: {}",
                        target.display()
                    )
                })?;
            } else {
                bail!(
                    "Cannot reset template '{}': unsupported entry type at {}",
                    name,
                    target.display()
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // 存在しない: そのまま fresh 展開へ
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "Failed to inspect existing template path before reset: {}",
                    target.display()
                )
            });
        }
    }

    // 単一ターゲット再展開。`extract_single_embedded_template_if_missing`
    // は既存 dir を保護するが、ここでは直前に削除したので fresh 展開が走る。
    crate::cli::run::template::extract_single_embedded_template_if_missing(config_dir, name)?;

    println!(
        "Template '{}' reset: fresh copy extracted to {}",
        name,
        target.display()
    );
    Ok(())
}

/// `vibepod template status`: print ecc-cache state.
pub fn status() -> Result<()> {
    let config_dir = config::default_config_dir()?;
    let cache = crate::ecc::cache_dir(&config_dir);

    let unified = config::load_unified(&config_dir)?;
    let ecc_cfg = unified.ecc.unwrap_or_default();

    println!(
        "ecc repo:         {}",
        sanitize_single_line(&ecc_cfg.repo, 500)
    );
    println!(
        "configured ref:   {}",
        sanitize_single_line(&ecc_cfg.r#ref, 200)
    );
    println!(
        "refresh_ttl:      {}",
        sanitize_single_line(&ecc_cfg.refresh_ttl, 50)
    );
    println!("auto_refresh:     {}", ecc_cfg.auto_refresh);
    println!("cache dir:        {}", cache.display());

    if !cache.join(".git").exists() {
        println!("cache status:     not initialized — run `vibepod init`");
        return Ok(());
    }

    let commit = match std::process::Command::new("git")
        .current_dir(&cache)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) => format!(
            "unknown (git rev-parse exited {}: {})",
            o.status,
            sanitize_single_line(&String::from_utf8_lossy(&o.stderr), 200)
        ),
        Err(e) => format!(
            "unknown (failed to run git: {})",
            sanitize_single_line(&format!("{e}"), 200)
        ),
    };
    println!("current commit:   {commit}");

    if let Some(age) = crate::ecc::cache_age_seconds(&config_dir) {
        let hours = age / 3600;
        let minutes = (age % 3600) / 60;
        println!("last updated:     {hours}h{minutes}m ago");
    }

    Ok(())
}

/// `vibepod template update [--ref <ref>]`: blocking fetch + reset of
/// the ecc cache. Optional `ref_override` overrides the configured ref
/// for this update only (does NOT persist to config.toml).
pub fn update(ref_override: Option<&str>) -> Result<()> {
    let config_dir = config::default_config_dir()?;
    let unified = config::load_unified(&config_dir)?;
    let mut ecc_cfg = unified.ecc.unwrap_or_default();
    if let Some(r) = ref_override {
        ecc_cfg.r#ref = r.to_string();
    }
    ecc_cfg.validate()?;

    let cache = crate::ecc::cache_dir(&config_dir);
    if !cache.join(".git").exists() {
        anyhow::bail!(
            "ecc cache not initialized at {}: run `vibepod init` first",
            cache.display()
        );
    }

    println!(
        "Fetching ecc ref '{}' into {}...",
        sanitize_single_line(&ecc_cfg.r#ref, 200),
        cache.display()
    );
    crate::ecc::fetch_latest(&config_dir, &ecc_cfg)?;
    println!("Updated.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded_name_or_skip() -> Option<String> {
        // v1.6 以降は <lang>/<mode> 形式のネストされた公式 bundle
        // (例: rust/impl, generic/review) のみが embed される。ネスト
        // container は CLAUDE.md を直下に持たないため、このテストは
        // CLAUDE.md を直下に持つ flat な埋め込み template のみを対象
        // にする (現状は該当なしで skip される想定だが、将来 flat な
        // embedded template が再追加された場合のガードとして残す)。
        use crate::cli::run::template::EMBEDDED_TEMPLATES;
        crate::cli::run::template::embedded_template_names()
            .into_iter()
            .find(|name| {
                EMBEDDED_TEMPLATES
                    .get_dir(name.as_str())
                    .map(|d| d.get_file("CLAUDE.md").is_some())
                    .unwrap_or(false)
            })
    }

    #[test]
    fn embedded_name_or_skip_matches_v1_6_invariant() {
        // v1.6 has zero flat embedded templates (all bundles are nested
        // `<lang>/<mode>`). If this assertion ever starts failing, it
        // means either (a) a flat template was added to
        // templates-data/, or (b) the path math in embedded_name_or_skip
        // was broken again. Either way, the test author must
        // investigate — do not reflexively update the assertion.
        let found = embedded_name_or_skip();
        assert_eq!(
            found, None,
            "v1.6 expected zero flat embedded templates, but found {:?}. \
             Either a new top-level template was added (update this test and \
             reset_in_* siblings) or embedded_name_or_skip's path math regressed.",
            found
        );
    }

    #[test]
    fn reset_in_rejects_non_embedded_name() {
        let config_dir = tempfile::tempdir().unwrap();
        let err = reset_in(config_dir.path(), "definitely-not-embedded", true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not an embedded template"),
            "expected non-embedded rejection, got: {}",
            msg
        );
    }

    #[test]
    fn reset_in_without_force_refuses_even_for_embedded() {
        let Some(name) = embedded_name_or_skip() else {
            return;
        };
        let config_dir = tempfile::tempdir().unwrap();
        let err = reset_in(config_dir.path(), &name, false).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--force"),
            "expected --force requirement, got: {}",
            msg
        );
    }

    #[test]
    fn reset_in_refuses_user_override_dir() {
        // embedded と同名の dir が marker 無しで存在する → user override
        // として扱って reset を拒否する。
        let Some(name) = embedded_name_or_skip() else {
            return;
        };
        let config_dir = tempfile::tempdir().unwrap();
        let target = config_dir.path().join("templates").join(&name);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("CLAUDE.md"), "user override").unwrap();
        // 明示的に marker は書かない

        let err = reset_in(config_dir.path(), &name, true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("user override"),
            "expected user override rejection, got: {}",
            msg
        );
        // user の内容が保持されていること (reset が走っていない)
        let content = std::fs::read_to_string(target.join("CLAUDE.md")).unwrap();
        assert_eq!(content, "user override");
    }

    #[test]
    fn reset_in_fresh_extract_when_target_absent() {
        // 既存 dir が無い状態でも reset は成功して fresh 展開が走る。
        let Some(name) = embedded_name_or_skip() else {
            return;
        };
        let config_dir = tempfile::tempdir().unwrap();
        assert!(!config_dir.path().join("templates").join(&name).exists());

        reset_in(config_dir.path(), &name, true).unwrap();

        let target = config_dir.path().join("templates").join(&name);
        assert!(target.is_dir());
        assert!(
            target.join(".vibepod-embedded").is_file(),
            "fresh extract should write the embedded marker"
        );
    }

    #[test]
    fn reset_in_replaces_prior_embedded_extract() {
        // 前回の embedded extract (marker あり) に対して reset すると、
        // 既存 dir が削除されて fresh 展開される。ユーザーが追加したファイル
        // も一緒に消えることを確認 (reset の責務)。
        let Some(name) = embedded_name_or_skip() else {
            return;
        };
        let config_dir = tempfile::tempdir().unwrap();
        let target = config_dir.path().join("templates").join(&name);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(".vibepod-embedded"), "0.0.0-test").unwrap();
        std::fs::write(target.join("STALE.md"), "should be gone after reset").unwrap();

        reset_in(config_dir.path(), &name, true).unwrap();

        assert!(
            !target.join("STALE.md").exists(),
            "stale file should be wiped"
        );
        assert!(
            target.join(".vibepod-embedded").is_file(),
            "marker should be re-written after fresh extract"
        );
        // fresh 展開後、embedded 側の CLAUDE.md が置かれている
        assert!(target.join("CLAUDE.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn reset_in_handles_symlink_target() {
        // 既存 target が (embedded dir へ向く) symlink の場合も、reset は
        // remove_file で消してから fresh 展開できる。
        // (symlink 経由だと is_embedded_extracted() は解決先の marker を見る)
        let Some(name) = embedded_name_or_skip() else {
            return;
        };
        let config_dir = tempfile::tempdir().unwrap();
        let templates = config_dir.path().join("templates");
        std::fs::create_dir_all(&templates).unwrap();

        // 解決先に marker 付きの dir を用意 (embedded extract の模倣)
        let real = templates.join("__real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join(".vibepod-embedded"), "0.0.0-test").unwrap();

        let link = templates.join(&name);
        std::os::unix::fs::symlink(&real, &link).unwrap();

        reset_in(config_dir.path(), &name, true).unwrap();

        // reset 後は symlink は消えて、fresh な dir が置かれている
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(!meta.file_type().is_symlink(), "symlink should be replaced");
        assert!(link.is_dir());
        assert!(link.join(".vibepod-embedded").is_file());
    }

    #[test]
    fn reset_in_rejects_regular_file_at_target_when_force_missing() {
        // regular file が target にある + force 無し → force 要求エラー
        // (is_embedded_extracted() は false なので user override 扱いで
        //  先に bail するのが正しい挙動)
        let Some(name) = embedded_name_or_skip() else {
            return;
        };
        let config_dir = tempfile::tempdir().unwrap();
        let templates = config_dir.path().join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join(&name), "not a dir").unwrap();

        let err = reset_in(config_dir.path(), &name, true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("user override"),
            "expected user override rejection (file has no marker), got: {}",
            msg
        );
    }
}
