//! Template mount switching logic.
//!
//! vibepod v2 では 「マウントするものを変える = モード切り替え」という
//! mechanism を採用している。本 module はその template 側（vibepod
//! 管理のテンプレート）のマウント構築を担当する。
//!
//! Phase 2 の時点では、`--template <name>` で明示指定された場合にのみ
//! template mount が使われる。指定が無い場合は v1.4.3 の host mount
//! 挙動のまま（後方互換）。`--prompt` 時の自動 default template 切替は
//! Phase 4 で `effective_template_name` を拡張して導入予定。

use anyhow::{bail, Result};
use std::path::Path;

use super::RunOptions;

/// 適用すべき template 名を決定する。
///
/// Phase 2 では `opts.template` の値をそのまま返すだけ。ユーザーが
/// 明示的に `--template <name>` を指定した場合のみ `Some` を返し、
/// それ以外（interactive も `--prompt` も）は `None` を返して
/// host mount path にフォールバックする。
///
/// Phase 4 で `opts.prompt.is_some()` の場合に config の
/// `default_prompt_template` を返すよう拡張する予定。
pub fn effective_template_name(opts: &RunOptions) -> Option<String> {
    opts.template.clone()
}

/// 有効な template 名であることを検証する。
///
/// Path traversal 攻撃（`../` で `~/.config/vibepod/templates/` 外に
/// 逃げる）を防ぐため、template 名は「空でない、かつ ASCII 英数字 /
/// ハイフン / アンダースコアのみ」を許可する。これで `.`, `/`, `\`,
/// 空白、制御文字などが全て弾かれる。
fn validate_template_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Template name must not be empty");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!(
            "Template name '{}' is invalid: only ASCII letters, digits, '-', and '_' are allowed",
            name
        );
    }
    Ok(())
}

/// 指定された template ディレクトリの中身をコンテナへのマウント
/// エントリに変換する。
///
/// Template ディレクトリは `<config_dir>/templates/<name>/` に配置される
/// 想定で、中身は host の `~/.claude/` と同じ構造を持てる:
///
/// - `CLAUDE.md`      → `/home/vibepod/.claude/CLAUDE.md`
/// - `skills/`        → `/home/vibepod/.claude/skills`
/// - `agents/`        → `/home/vibepod/.claude/agents`
/// - `plugins/`       → `/home/vibepod/.claude/plugins`
/// - `settings.json`  → `/home/vibepod/.claude/settings.json`
///
/// 存在するエントリだけがマウント対象になる。template ディレクトリ
/// そのものが存在しない場合はエラー。
pub fn build_template_mounts(
    template_name: &str,
    config_dir: &Path,
) -> Result<Vec<(String, String)>> {
    validate_template_name(template_name)?;

    let template_dir = config_dir.join("templates").join(template_name);
    if !template_dir.is_dir() {
        bail!(
            "Template '{}' not found at {}",
            template_name,
            template_dir.display()
        );
    }

    let mut mounts = Vec::new();

    let claude_md = template_dir.join("CLAUDE.md");
    if claude_md.is_file() {
        mounts.push((
            claude_md.to_string_lossy().to_string(),
            "/home/vibepod/.claude/CLAUDE.md".to_string(),
        ));
    }

    let skills_dir = template_dir.join("skills");
    if skills_dir.is_dir() {
        mounts.push((
            skills_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/skills".to_string(),
        ));
    }

    let agents_dir = template_dir.join("agents");
    if agents_dir.is_dir() {
        mounts.push((
            agents_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/agents".to_string(),
        ));
    }

    let plugins_dir = template_dir.join("plugins");
    if plugins_dir.is_dir() {
        // Phase 2 note: template の plugins は単一マウントで
        // /home/vibepod/.claude/plugins にのみ配置する。host mode の
        // `plugins_mount_entries` が行う「絶対パス (`installed_plugins.json` の
        // `installPath`) への二重マウント」は行わない。
        //
        // template 配布物に `installed_plugins.json` が含まれ、その `installPath`
        // が build-time の絶対パスだった場合、container 内では解決できない。
        // Phase 3/4 で embed 機構と実 template を入れた際に以下のいずれかで解決する予定:
        //   a) template 配布前に `installed_plugins.json` の `installPath` を
        //      コンテナ側の固定パス (/home/vibepod/.claude/plugins/cache/...) に
        //      normalize する
        //   b) template メタデータで必要な plugin set を宣言し、container 内で
        //      再 install する
        //
        // Phase 2 の時点では実 template 内容が無いので runtime 影響なし。
        mounts.push((
            plugins_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/plugins".to_string(),
        ));
    }

    let settings_json = template_dir.join("settings.json");
    if settings_json.is_file() {
        mounts.push((
            settings_json.to_string_lossy().to_string(),
            "/home/vibepod/.claude/settings.json".to_string(),
        ));
    }

    // template ディレクトリは存在するが、中身が全て空の場合はエラー。
    // 「空の template を渡して何も mount されないこと」は呼び出し側の
    // バグを示唆するので、早期に落としたほうが safe。
    if mounts.is_empty() {
        bail!(
            "Template '{}' at {} is empty (no CLAUDE.md, skills/, agents/, plugins/, or settings.json found)",
            template_name,
            template_dir.display()
        );
    }

    Ok(mounts)
}
