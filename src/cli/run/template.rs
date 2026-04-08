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

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use std::path::{Path, PathBuf};

use super::RunOptions;

/// ビルド時に `templates-data/` 配下全体をバイナリに埋め込む。
///
/// ここに置かれたサブディレクトリが vibepod の「公式 template」となり、
/// 初回 `vibepod run` または `vibepod template list` 時に
/// `~/.config/vibepod/templates/<name>/` に展開される（既存ディレクトリ
/// があればユーザー編集を保護するため展開しない）。
///
/// Phase 3 の時点では `templates-data/` は空（`.gitkeep` のみ）。
/// 実際の公式 template (rust-code / review) は Phase 4 で追加される。
pub static EMBEDDED_TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates-data");

/// 適用すべき template 名を決定する。
///
/// 優先順位:
/// 1. `opts.template` が `Some` → そのまま使う（ユーザー明示指定）
/// 2. `opts.prompt` が `Some` かつ `config.default_prompt_template()`
///    が `Some` → config で指定されたデフォルトを使う（`vibepod template
///    set-default <name>` で設定される値）
/// 3. それ以外（interactive / template 未設定） → `None` を返して
///    host mount path にフォールバックする（v1.4.3 互換挙動）
///
/// 注意: 2. が効くのは `--prompt` mode だけ。interactive でも
/// `--template` 未指定なら host mount のまま。これは interactive が
/// 「ユーザー個人環境を使う」前提で、default template のような
/// opinionated な切替は `--prompt` autonomous 実行にだけ効かせたい
/// ため。
pub fn effective_template_name(
    opts: &RunOptions,
    config: &crate::config::VibepodConfig,
) -> Option<String> {
    if let Some(name) = &opts.template {
        return Some(name.clone());
    }
    // `--worktree` 指定時は default template を適用しない。
    // `--worktree` と template モードの併用は Phase 2 で明示的に拒否
    // しているため (`prepare_context` の guard 参照)、config による
    // 暗黙切替で worktree+template 組み合わせに入ってしまうのを防ぐ。
    if opts.prompt.is_some() && !opts.worktree {
        if let Some(default) = config.default_prompt_template() {
            return Some(default);
        }
    }
    None
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

/// Template 名を検証し、その template ディレクトリの canonical path を返す。
///
/// この関数は path traversal / symlink escape 対策の要:
/// - 名前を英数字 + `-` + `_` に制限
/// - ディレクトリを canonicalize し、`<config_dir>/templates/` 配下で
///   あることを verify（symlinked template dir が外を指す場合は reject）
/// - 返す path は canonical（macOS case-insensitive FS で "review"/"Review"
///   が同じ canonical path に解決されるため、container name hash 等の
///   stable key としても使える）
pub fn resolve_template_dir(template_name: &str, config_dir: &Path) -> Result<PathBuf> {
    validate_template_name(template_name)?;

    let templates_root = config_dir.join("templates");
    let template_dir = templates_root.join(template_name);
    if !template_dir.is_dir() {
        bail!(
            "Template '{}' not found at {}",
            template_name,
            template_dir.display()
        );
    }

    let canonical_template = template_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize template directory: {}",
            template_dir.display()
        )
    })?;

    // templates_root 自体が存在しない場合は上の `template_dir.is_dir()` で
    // 既に弾かれているので、ここでは必ず存在する。canonical を取って
    // containment チェックする。
    let canonical_root = templates_root.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize templates root: {}",
            templates_root.display()
        )
    })?;

    if !canonical_template.starts_with(&canonical_root) {
        bail!(
            "Template '{}' resolves to {} which is outside {} (possible symlink escape)",
            template_name,
            canonical_template.display(),
            canonical_root.display()
        );
    }

    Ok(canonical_template)
}

/// `template_dir` 配下にある `entry` が存在し、symlink で外部を指して
/// いない場合のみ canonical path を返す。存在しなければ `Ok(None)`、
/// symlink escape なら `Err`。
fn resolve_template_entry(
    template_dir: &Path,
    entry: &str,
    expect_dir: bool,
) -> Result<Option<PathBuf>> {
    let path = template_dir.join(entry);
    let exists = if expect_dir {
        path.is_dir()
    } else {
        path.is_file()
    };
    if !exists {
        return Ok(None);
    }

    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize template entry: {}", path.display()))?;

    // template_dir は既に canonical である前提で、entry の canonical が
    // その配下にあることを verify（symlink が template root 外を指す
    // 攻撃を防ぐ）
    if !canonical.starts_with(template_dir) {
        bail!(
            "Template entry {} resolves to {} which is outside template root {} (possible symlink escape)",
            path.display(),
            canonical.display(),
            template_dir.display()
        );
    }

    Ok(Some(canonical))
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
/// 存在するエントリだけがマウント対象になる。全てのエントリは
/// canonicalize され、template root の外を指す symlink は reject される。
pub fn build_template_mounts(
    template_name: &str,
    config_dir: &Path,
) -> Result<Vec<(String, String)>> {
    let template_dir = resolve_template_dir(template_name, config_dir)?;

    let mut mounts = Vec::new();

    if let Some(canonical) = resolve_template_entry(&template_dir, "CLAUDE.md", false)? {
        mounts.push((
            canonical.to_string_lossy().to_string(),
            "/home/vibepod/.claude/CLAUDE.md".to_string(),
        ));
    }

    if let Some(canonical) = resolve_template_entry(&template_dir, "skills", true)? {
        mounts.push((
            canonical.to_string_lossy().to_string(),
            "/home/vibepod/.claude/skills".to_string(),
        ));
    }

    if let Some(canonical) = resolve_template_entry(&template_dir, "agents", true)? {
        mounts.push((
            canonical.to_string_lossy().to_string(),
            "/home/vibepod/.claude/agents".to_string(),
        ));
    }

    if let Some(plugins_dir) = resolve_template_entry(&template_dir, "plugins", true)? {
        // Phase 2 では `installed_plugins.json` を含む plugins 構成は
        // サポートしない。理由:
        //
        // host mode の `plugins_mount_entries` は plugins ディレクトリを
        // `/home/vibepod/.claude/plugins` と `<host_home>/.claude/plugins` の
        // 2 箇所に bind mount することで、`installed_plugins.json` 内の絶対
        // パス (`installPath`) を container 内で解決している。
        //
        // template 側では build-time の絶対パスが container 内では存在
        // しないため、単純に `/home/vibepod/.claude/plugins` に 1 度だけ
        // bind mount しても Claude が `installPath` を解決できず silent に
        // 壊れる。
        //
        // Phase 3/4 で以下のいずれかで解決する予定:
        //   a) template build 時に `installed_plugins.json` の `installPath`
        //      を container 側の固定パス (/home/vibepod/.claude/plugins/...)
        //      に normalize する
        //   b) template メタデータで必要な plugin set を宣言し、container
        //      起動時に再 install する
        //
        // それまでは明示的にエラーにして silent breakage を防ぐ。
        // `plugins/` 配下に `installed_plugins.json` が無い場合は
        // シンプルな直置きプラグイン（plain files）として単一 mount を
        // 許可する。
        let installed_plugins_json = plugins_dir.join("installed_plugins.json");
        if installed_plugins_json.is_file() {
            bail!(
                "Template '{}' ships plugins/installed_plugins.json, which is not \
                 supported yet (tracked for Phase 3/4). Template plugins with an \
                 installed_plugins.json registry cannot resolve their absolute \
                 installPath values inside the container. Remove installed_plugins.json \
                 or wait for Phase 3/4 template support.",
                template_name
            );
        }

        mounts.push((
            plugins_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/plugins".to_string(),
        ));
    }

    if let Some(canonical) = resolve_template_entry(&template_dir, "settings.json", false)? {
        mounts.push((
            canonical.to_string_lossy().to_string(),
            "/home/vibepod/.claude/settings.json".to_string(),
        ));
    }

    // Note: template ディレクトリが存在してさえいれば、中身が 0 件でも
    // valid（空の mounts を返す）。これは `--template blank` のようにして
    // 「ホスト ~/.claude を一切 mount しない = 素の Claude 環境で走らせる」
    // という明示的な opt-out パターンを許可するため。
    Ok(mounts)
}

/// 埋め込まれた公式 template の名前一覧を返す（トップレベルのサブ
/// ディレクトリ名のみ）。`.gitkeep` 等のファイルは除外する。
pub fn embedded_template_names() -> Vec<String> {
    let mut names: Vec<String> = EMBEDDED_TEMPLATES
        .dirs()
        .filter_map(|d| {
            d.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .filter(|n| validate_template_name(n).is_ok())
        .collect();
    names.sort();
    names
}

/// ユーザー追加 template の名前一覧を返す。
///
/// `<config_dir>/templates/` 配下のサブディレクトリ名を列挙し、
/// template 名として有効なもの（validate_template_name に通るもの）
/// だけを返す。ディレクトリが存在しない場合は空配列。
///
/// `resolve_template_dir` と同じ採否基準を適用する: in-root への
/// symlinked dir は valid として含める一方、外部を指す symlink は
/// 除外する。これによって `template list` / `set-default` の見える
/// 集合が `run --template` の実行可能集合と一致する（不一致だと
/// list には出ないが run は通る、または逆、という混乱が起きる）。
pub fn user_template_names(config_dir: &Path) -> Vec<String> {
    let templates_root = config_dir.join("templates");
    if !templates_root.is_dir() {
        return Vec::new();
    }
    let mut names: Vec<String> = std::fs::read_dir(&templates_root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        // `std::fs::metadata(path)` は symlink を辿る (DirEntry::metadata
        // は辿らない点に注意)。symlinked dir も is_dir として拾うために
        // path 経由で stat を取る。
        .filter(|e| {
            std::fs::metadata(e.path())
                .map(|m| m.is_dir())
                .unwrap_or(false)
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| validate_template_name(n).is_ok())
        // 最終的な escape チェックは resolve_template_dir に委譲する。
        // これで in-root symlink は通り、外部を指す symlink は弾かれる。
        .filter(|n| resolve_template_dir(n, config_dir).is_ok())
        .collect();
    names.sort();
    names
}

/// 埋め込み template のうち、ユーザー template ディレクトリに
/// まだ展開されていないものを `<config_dir>/templates/<name>/` に
/// コピーする。既存ディレクトリがあれば触らない（ユーザー編集の保護）。
///
/// 冪等: 既に展開済みの template はスキップされる。新規 vibepod
/// バージョンで embed template が更新されても、ユーザー既存 dir は
/// 上書きされない（明示的な再展開手段は v2.x で別途検討）。
pub fn extract_embedded_templates_if_missing(config_dir: &Path) -> Result<()> {
    // embed が空の場合は何もしない。これにより host mode 専用ユーザーの
    // read-only `~/.config/vibepod/` setup で不要な write を発生させない。
    if EMBEDDED_TEMPLATES.dirs().next().is_none() {
        return Ok(());
    }

    let templates_root = config_dir.join("templates");
    if !templates_root.exists() {
        std::fs::create_dir_all(&templates_root).with_context(|| {
            format!(
                "Failed to create templates root: {}",
                templates_root.display()
            )
        })?;
    }

    for embedded in EMBEDDED_TEMPLATES.dirs() {
        let name = match embedded.path().file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if validate_template_name(name).is_err() {
            // 不正な名前の embed entry（ビルド時のミス）は skip
            continue;
        }

        let dest = templates_root.join(name);

        // symlink を follow しない判定を使う。これにより
        // `templates/<name>` が外部ディレクトリへの symlink の場合も
        // 「正しい user template」として扱わず、明示的にエラーにする。
        // そうしないと `template list` は embed を広告するのに
        // `vibepod run --template <name>` は resolve_template_dir の
        // symlink escape チェックで失敗し、CLI が自己矛盾する。
        match std::fs::symlink_metadata(&dest) {
            Ok(meta) => {
                let ft = meta.file_type();
                if ft.is_symlink() {
                    bail!(
                        "Cannot extract embedded template '{}': {} is a symlink, \
                         which conflicts with the embedded template of the same name. \
                         Remove or rename the symlink (it will be rejected as symlink \
                         escape at runtime anyway).",
                        name,
                        dest.display()
                    );
                }
                if ft.is_dir() {
                    // 通常ディレクトリ: ユーザー編集を上書きしない
                    continue;
                }
                // regular file or その他
                bail!(
                    "Cannot extract embedded template '{}': {} exists but is not a directory. \
                     Remove or rename it to let vibepod materialize the embedded template.",
                    name,
                    dest.display()
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 存在しない → 下の extract_template_dir で展開する
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("Failed to stat template destination {}", dest.display())
                });
            }
        }
        extract_template_dir(embedded, &dest).with_context(|| {
            format!(
                "Failed to extract embedded template '{}' to {}",
                name,
                dest.display()
            )
        })?;
    }
    Ok(())
}

/// `include_dir::Dir` を指定されたパスに再帰的に展開する内部ヘルパー。
///
/// # 実行権限の扱い
///
/// `include_dir` クレートは埋め込み時にファイルの POSIX mode を保存
/// しないため、展開後のファイルは umask 準拠のデフォルト（通常 0644）
/// になる。template 内に実行可能ファイル（hook script 等）がある場合、
/// そのままでは実行できない。
///
/// Phase 3 では以下のヒューリスティックで救済する:
/// - ファイル名が `.sh` / `.bash` / `.zsh` / `.fish` で終わる
/// - ファイルがディレクトリ `bin/` / `scripts/` 配下にある
///
/// これらに該当する場合は `0755` を設定する。それ以外は umask 任せ。
/// 将来的に他パターンが必要になったら拡張するか、template 側に
/// `.vibepod-executable` のような metadata ファイルで宣言させる仕組みを
/// 入れる。
fn extract_template_dir(dir: &Dir<'_>, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("Failed to create directory: {}", dest.display()))?;

    for file in dir.files() {
        let file_name = match file.path().file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let file_path = dest.join(file_name);
        std::fs::write(&file_path, file.contents())
            .with_context(|| format!("Failed to write {}", file_path.display()))?;

        #[cfg(unix)]
        {
            if should_be_executable(file.path()) {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&file_path)
                    .with_context(|| format!("Failed to read metadata of {}", file_path.display()))?
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&file_path, perms).with_context(|| {
                    format!("Failed to set exec bits on {}", file_path.display())
                })?;
            }
        }
    }

    for subdir in dir.dirs() {
        let sub_name = match subdir.path().file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let sub_path = dest.join(sub_name);
        extract_template_dir(subdir, &sub_path)?;
    }

    Ok(())
}

/// Phase 3 heuristic: 展開されたファイルに実行権限を付けるべきか。
///
/// extension または親ディレクトリ名で判定する。
#[cfg(unix)]
fn should_be_executable(path: &Path) -> bool {
    // 拡張子判定
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if matches!(ext, "sh" | "bash" | "zsh" | "fish") {
            return true;
        }
    }
    // ディレクトリ名判定
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            if let Some(s) = name.to_str() {
                if matches!(s, "bin" | "scripts") {
                    return true;
                }
            }
        }
    }
    false
}
