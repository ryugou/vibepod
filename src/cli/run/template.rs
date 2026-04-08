//! Template mount switching logic.
//!
//! vibepod v2 では 「マウントするものを変える = モード切り替え」という
//! mechanism を採用している。本 module はその template 側（vibepod
//! 管理のテンプレート）のマウント構築を担当する。
//!
//! `--template <name>` で明示指定された場合に template mount が使われる。
//! 指定が無い場合は v1.4.3 の host mount 挙動のまま（後方互換）。
//! Phase 3 以降は `--prompt` で `--template` 未指定のとき、
//! `~/.config/vibepod/config.toml` の `[run] default_prompt_template` を
//! 見て自動的に template mount に切り替える (best-effort; 解決失敗時は
//! host mount にフォールバック)。

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use std::path::{Path, PathBuf};

use super::RunOptions;

/// ビルド時に `templates-data/` 配下全体をバイナリに埋め込む。
///
/// ここに置かれたサブディレクトリが vibepod の「公式 template」となり、
/// `vibepod run --template <name>` で template mode が要求され、かつ
/// 該当 template が `~/.config/vibepod/templates/<name>/` に見当たらない
/// ときに lazy 展開される (既存ディレクトリがあればユーザー編集を
/// 保護するため上書きしない)。`vibepod template list` / `template
/// set-default` は列挙のみで展開は行わないため、read-only な
/// `~/.config/vibepod/` setup を壊さない。
///
/// Phase 4 で公式 template (`rust-code` / `review`) が追加された。
pub static EMBEDDED_TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates-data");

/// 適用すべき template 名を決定する。
///
/// 優先順位:
/// 1. `opts.template` が `Some` → そのまま使う（ユーザー明示指定）。
///    存在チェックはここでは行わない: 明示指定はユーザー意図なので、
///    後段で「Template not found」エラーを出して fail-fast したい。
/// 2. `opts.prompt` が `Some` かつ `opts.worktree` が `false` かつ
///    `config.default_prompt_template()` が `Some` → config で指定された
///    デフォルトを使う（`vibepod template set-default <name>`）。
///    **ただし** その template が embedded / user いずれにも存在しない
///    場合は `None` を返してホストマウントにフォールバックする
///    （config はあくまで best-effort なので、未展開・未配置の template
///    名で run が壊れるのを防ぐ）。
/// 3. それ以外（interactive / worktree / 該当 template なし） → `None`
///    を返して host mount path にフォールバックする（v1.4.3 互換挙動）。
///
/// 注意:
///
/// 上記 2 番のルールが効くのは `--prompt` mode だけ。interactive でも
/// `--template` 未指定なら host mount のまま。これは interactive が
/// 「ユーザー個人環境を使う」前提で、default template のような
/// opinionated な切替は `--prompt` autonomous 実行にだけ効かせたいため。
///
/// `--worktree` 指定時は default template も適用しない。`--worktree`
/// と template モードの併用は Phase 2 で明示的に拒否しているため
/// (`prepare_context` の guard 参照)、config による暗黙切替で
/// worktree+template 組み合わせに入ってしまうのを防ぐ。
pub fn effective_template_name(
    opts: &RunOptions,
    config: &crate::config::VibepodConfig,
    config_dir: &Path,
) -> Option<String> {
    if let Some(name) = &opts.template {
        return Some(name.clone());
    }
    if opts.prompt.is_some() && !opts.worktree {
        if let Some(default) = config.default_prompt_template() {
            // Best-effort 解決:
            //
            //   1. まず on-disk の `templates/<default>/` を直接 resolve
            //      する。ユーザーが管理する template (embedded を一切
            //      使わないケースを含む) は extract に依存せずそのまま
            //      使えるべき。embedded extraction の失敗が user-managed
            //      default を巻き込んで host mount フォールバックさせる
            //      regression を防ぐ。
            //   2. 直接 resolve に失敗した場合、その名前が embedded 集合に
            //      あるなら lazy extract を試み、再 resolve する。
            //   3. それでも resolve できなければ host mount フォールバック
            //      (`None` を返す)。default は best-effort なので、
            //      ユーザーが設定したからといって prompt run を壊さない。
            //
            // 明示的な `--template` (上の `opts.template` 分岐) は
            // この best-effort 扱いを受けず、`prepare_context` の後段で
            // resolve に失敗すれば fail-fast する。これはユーザーの
            // 明示的意図なのでエラーが見えるべき。
            if resolve_template_dir(&default, config_dir).is_ok() {
                return Some(default);
            }
            if embedded_template_names().iter().any(|n| n == &default)
                && extract_single_embedded_template_if_missing(config_dir, &default).is_ok()
                && resolve_template_dir(&default, config_dir).is_ok()
            {
                return Some(default);
            }
            // Fallback: config で設定された default が解決できなかった。
            // host mount で prompt run を続行するが、silently fall back
            // すると「template list では default になっているのに run で
            // 効かない」という不可解な状態になるため、stderr に警告を
            // 出してから None を返す。明示的な `--template <name>` の
            // fail-fast とは違い、ここはあくまで best-effort の default
            // 適用なので run 自体は止めない。
            eprintln!(
                "warning: configured default template '{}' could not be resolved; \
                 falling back to host mount. Check \
                 `~/.config/vibepod/templates/{}` for a missing / conflicting entry, \
                 run `vibepod template list` to see what is available, or replace \
                 the default with a valid name via \
                 `vibepod template set-default <name>` (or edit the \
                 `[run] default_prompt_template` line in \
                 `~/.config/vibepod/config.toml` to remove it).",
                default, default
            );
            return None;
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
/// だけを返す。ディレクトリが存在しない場合は空配列を返す（正常）。
///
/// `resolve_template_dir` と同じ採否基準を適用する: in-root への
/// symlinked dir は valid として含める一方、外部を指す symlink は
/// 除外する。これによって `template list` / `set-default` の見える
/// 集合が `run --template` の実行可能集合と一致する（不一致だと
/// list には出ないが run は通る、または逆、という混乱が起きる）。
///
/// エラー処理: `read_dir` の I/O エラーはそのまま伝播する。
/// `~/.config/vibepod/templates/` が存在するのに読めないような状況
/// (パーミッション破壊、I/O 故障) は silent に無視せず、CLI で
/// 「I/O failure」をユーザーに見せるのが正しい (silent な不一致は
/// `set-default` が実在 template を reject する不可解な挙動を生む)。
/// 個別エントリの metadata 取得失敗だけは「読めないエントリ = 一覧
/// から除外」として best-effort で扱う (read_dir 自体は成功している
/// ので catastrophic ではない)。
pub fn user_template_names(config_dir: &Path) -> Result<Vec<String>> {
    let templates_root = config_dir.join("templates");
    if !templates_root.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&templates_root).with_context(|| {
        format!(
            "Failed to read templates directory: {}",
            templates_root.display()
        )
    })?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "Failed to read entry under templates directory: {}",
                templates_root.display()
            )
        })?;
        // `std::fs::metadata(path)` は symlink を辿る (DirEntry::metadata
        // は辿らない点に注意)。symlinked dir も is_dir として拾うために
        // path 経由で stat を取る。symlink 解決失敗 (broken symlink) は
        // best-effort で除外。
        let is_dir = match std::fs::metadata(entry.path()) {
            Ok(m) => m.is_dir(),
            Err(_) => false,
        };
        if !is_dir {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if validate_template_name(&name).is_err() {
            continue;
        }
        // 最終的な escape チェックは resolve_template_dir に委譲する。
        if resolve_template_dir(&name, config_dir).is_err() {
            continue;
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// 埋め込み template のうち、ユーザー template ディレクトリに
/// まだ展開されていないものを `<config_dir>/templates/<name>/` に
/// コピーする。既存ディレクトリがあれば触らない（ユーザー編集の保護）。
///
/// 冪等: 既に展開済みの template はスキップされる。新規 vibepod
/// バージョンで embed template が更新されても、ユーザー既存 dir は
/// 上書きされない（明示的な再展開手段は v2.x で別途検討）。
///
/// **注意**: この関数は全 embedded template に対して処理を行うため、
/// 1 つでも conflict があると残りの template も展開されない。単一
/// template だけを展開したい場合 (典型的には `vibepod run --template X`
/// の経路) は `extract_single_embedded_template_if_missing` を使うこと。
pub fn extract_embedded_templates_if_missing(config_dir: &Path) -> Result<()> {
    // embed が空の場合は何もしない。これにより host mode 専用ユーザーの
    // read-only `~/.config/vibepod/` setup で不要な write を発生させない。
    if EMBEDDED_TEMPLATES.dirs().next().is_none() {
        return Ok(());
    }

    ensure_templates_root(config_dir)?;

    for embedded in EMBEDDED_TEMPLATES.dirs() {
        let name = match embedded.path().file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if validate_template_name(name).is_err() {
            // 不正な名前の embed entry（ビルド時のミス）は skip
            continue;
        }
        extract_one_embedded(embedded, name, config_dir)?;
    }
    Ok(())
}

/// 特定の 1 つの embedded template だけを lazy 展開する。
///
/// `extract_embedded_templates_if_missing` と違い、**他の embedded
/// template に conflict や破損があっても、要求された 1 つだけを展開**
/// する。これにより `vibepod run --template rust-code` が
/// 無関係な `review` dir の破損で失敗する regression を防ぐ。
///
/// `name` が embedded 集合に存在しなければ `Ok(())` を返す (呼び出し側
/// がすでに存在確認しているか、user-provided だった場合に no-op で通す)。
pub fn extract_single_embedded_template_if_missing(config_dir: &Path, name: &str) -> Result<()> {
    if validate_template_name(name).is_err() {
        return Ok(());
    }
    // 対象を embed から 1 つだけ拾う
    let embedded = match EMBEDDED_TEMPLATES.dirs().find(|d| {
        d.path()
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s == name)
            .unwrap_or(false)
    }) {
        Some(d) => d,
        None => return Ok(()),
    };

    ensure_templates_root(config_dir)?;
    extract_one_embedded(embedded, name, config_dir)
}

fn ensure_templates_root(config_dir: &Path) -> Result<()> {
    let templates_root = config_dir.join("templates");
    match std::fs::symlink_metadata(&templates_root) {
        Ok(meta) => {
            // 存在する: ディレクトリ (or ディレクトリに解決される symlink) で
            // あることを確認する。ファイル・壊れた symlink などの場合は
            // 後続の extract が「Not a directory」等の曖昧なエラーになるため、
            // ここで明示的に bail する。
            let ft = meta.file_type();
            if ft.is_dir() {
                return Ok(());
            }
            if ft.is_symlink() {
                // symlink の場合は最終的な解決先が dir かどうかで判断。
                if std::fs::metadata(&templates_root)
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
                {
                    return Ok(());
                }
                bail!(
                    "Templates root exists but is a symlink that does not resolve \
                     to a directory: {}",
                    templates_root.display()
                );
            }
            bail!(
                "Templates root exists but is not a directory: {}",
                templates_root.display()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&templates_root).with_context(|| {
                format!(
                    "Failed to create templates root: {}",
                    templates_root.display()
                )
            })?;
            Ok(())
        }
        Err(e) => Err(e).with_context(|| {
            format!(
                "Failed to stat templates root: {}",
                templates_root.display()
            )
        }),
    }
}

/// 単一 embedded entry を `<config_dir>/templates/<name>/` に展開する
/// 内部ヘルパー。既存・symlink・conflict などの前置判定と atomic rename
/// を担当する。`extract_embedded_templates_if_missing` からループで、
/// `extract_single_embedded_template_if_missing` から 1 回だけ呼ばれる。
fn extract_one_embedded(embedded: &Dir<'_>, name: &str, config_dir: &Path) -> Result<()> {
    let templates_root = config_dir.join("templates");
    let dest = templates_root.join(name);

    // 既存エントリの判定。優先順位:
    //   1. 何もない (NotFound) → 後段で extract
    //   2. ディレクトリ実体 (regular dir) → ユーザー編集として保護、skip
    //   3. in-root を指す symlink → ユーザー override として保護、skip
    //      (resolve_template_dir も同条件で受け入れるので CLI 全体で
    //      整合する)
    //   4. 外部を指す symlink / regular file / その他 → 明示的にエラー
    match std::fs::symlink_metadata(&dest) {
        Ok(meta) => {
            let ft = meta.file_type();
            if ft.is_dir() {
                // 通常ディレクトリ: ユーザー編集を上書きしない
                return Ok(());
            }
            if ft.is_symlink() {
                // resolve_template_dir と同じルールで受け入れ判定する。
                // in-root に解決される symlink は user override として
                // 尊重し、extraction を skip する。out-of-root のものや
                // 解決失敗するものは bail して、ユーザーに対処を促す。
                if resolve_template_dir(name, config_dir).is_ok() {
                    return Ok(());
                }
                bail!(
                    "Cannot extract embedded template '{}': {} is a symlink that \
                     escapes the templates root or cannot be resolved. \
                     Remove or rename the symlink so vibepod can materialize \
                     the embedded template.",
                    name,
                    dest.display()
                );
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

    // 部分展開で「中途半端な dir が残ったまま skip され続ける」事故を
    // 防ぐため、まず兄弟ディレクトリ `<name>.tmp-<pid>` に展開して
    // 完了後に rename で原子的に dest に移す。途中で失敗した場合は
    // tmp dir を best-effort で削除し、次回呼び出し時に dest が無い
    // 状態に戻す (rename 前なので「ある or ない」しか観測されない)。
    let tmp_dest = templates_root.join(format!("{}.tmp-{}", name, std::process::id()));
    if tmp_dest.exists() {
        // 過去 run の残骸 (同 PID 衝突は事実上無視できるが念のため)
        let _ = std::fs::remove_dir_all(&tmp_dest);
    }
    if let Err(err) = extract_template_dir(embedded, &tmp_dest).with_context(|| {
        format!(
            "Failed to extract embedded template '{}' to staging dir {}",
            name,
            tmp_dest.display()
        )
    }) {
        let _ = std::fs::remove_dir_all(&tmp_dest);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&tmp_dest, &dest) {
        // 並列実行の race: 他 process が先に install を終えて dest
        // が既に存在する場合、rename は `AlreadyExists` または
        // `DirectoryNotEmpty` で失敗する。期待する最終状態 (dest に
        // template がある) は満たされているので、自分の staging dir
        // だけ掃除して成功扱いにする。
        let rename_conflict = matches!(
            err.kind(),
            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
        );
        let _ = std::fs::remove_dir_all(&tmp_dest);
        if rename_conflict && dest.is_dir() {
            return Ok(());
        }
        return Err(err).with_context(|| {
            format!(
                "Failed to install embedded template '{}' from {} to {}",
                name,
                tmp_dest.display(),
                dest.display()
            )
        });
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
/// `template list` が「embedded を vibepod が展開した dir」と
/// 「ユーザー自身が同名で作った dir (override)」を区別するためのマーカー
/// ファイル名。中身は extract 時の vibepod バージョン (informational)。
/// マーカーは extract のトップ階層にのみ書き、再帰展開時には書かない
/// (ネストしたディレクトリは template の一部であって、独立した template
/// ではないため)。
pub const EMBEDDED_MARKER_FILENAME: &str = ".vibepod-embedded";

/// 指定された template ディレクトリが vibepod の embed 展開によって
/// 作られたものかどうかを判定する。マーカーファイルの存在のみで判定し、
/// 内容は問わない (将来の version 比較フィールド拡張用)。
pub fn is_embedded_extracted(template_dir: &Path) -> bool {
    template_dir.join(EMBEDDED_MARKER_FILENAME).is_file()
}

fn extract_template_dir(dir: &Dir<'_>, dest: &Path) -> Result<()> {
    extract_template_dir_inner(dir, dest, true)
}

fn extract_template_dir_inner(dir: &Dir<'_>, dest: &Path, is_top: bool) -> Result<()> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("Failed to create directory: {}", dest.display()))?;

    if is_top {
        // トップ階層にだけマーカーを書く。`template list` がこのファイルを
        // 見て embedded vs user-override を判定する。
        let marker_path = dest.join(EMBEDDED_MARKER_FILENAME);
        std::fs::write(&marker_path, env!("CARGO_PKG_VERSION"))
            .with_context(|| format!("Failed to write {}", marker_path.display()))?;
    }

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
        // 再帰呼び出しでは marker を書かない (ネスト dir は独立 template
        // ではない)
        extract_template_dir_inner(subdir, &sub_path, false)?;
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
