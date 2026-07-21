use anyhow::{Context, Result};

use crate::config;
use crate::runtime::{ContainerConfig, ContainerStatus};
use crate::session::SessionStore;

mod interactive;
pub mod lock;
pub mod prepare;
mod prompt;
pub mod template;

/// CLI `run` サブコマンドのオプション
///
/// `vibepod run` コマンドの全引数を保持する。
pub struct RunOptions {
    pub resume: bool,
    pub prompt: Option<String>,
    pub no_network: bool,
    pub env_vars: Vec<String>,
    pub env_file: Option<String>,
    pub lang: Option<String>,
    pub worktree: bool,
    pub mount: Vec<String>,
    /// `--new` フラグ: 既存コンテナを破棄して新規作成する
    pub new_container: bool,
    /// `--template <name>` フラグ: vibepod 管理の template を
    /// `/home/vibepod/.claude/` にマウントする。未指定時は host の
    /// `~/.claude/` をマウントする（v1.4.3 互換挙動）
    pub template: Option<String>,
    /// `--mode` フラグ: `impl`（デフォルト、コード編集）または `review`（読み取り専用レビュー）。
    /// 現時点では主に実行モードの分岐（permissions.deny の適用や template 選択）に使用する。
    pub mode: crate::cli::RunMode,
    /// コンテナ内 Claude Code の更新チェック方針（`--update` / `--no-update`）。
    pub update_policy: crate::update::UpdatePolicy,
}

pub(super) struct RunContext {
    pub(super) container_name: String,
    pub(super) effective_workspace: String,
    pub(super) claude_args: Vec<String>,
    /// ユーザー環境変数（コンテナ作成時に渡す）
    pub(super) resolved_env_vars: Vec<String>,
    /// 認証トークン（`docker exec -e` で毎回渡す）
    pub(super) exec_env_vars: Vec<String>,
    pub(super) setup_cmd: Option<String>,
    pub(super) temp_claude_json: Option<std::path::PathBuf>,
    /// Per-container runtime directory under
    /// `<config_dir>/runtime/<container_name>/`. All vibepod-managed runtime
    /// files for this container (temp claude.json copy, sanitized
    /// settings.json, etc.) live under this path. Used for cleanup of
    /// disposable containers regardless of which artifacts were created.
    pub(super) runtime_dir: std::path::PathBuf,
    /// vibepod のグローバル設定ディレクトリ（通常 `~/.config/vibepod`）。
    /// 更新チェックのタイムスタンプ (`update-check.json`) の保存先として使う。
    pub(super) config_dir: std::path::PathBuf,
    pub(super) global_config: config::GlobalConfig,
    pub(super) home: std::path::PathBuf,
    pub(super) worktree_branch_name: Option<String>,
    pub(super) worktree_dir_name: Option<String>,
    pub(super) lang_display: String,
    /// Sorted, deduped list of language identifiers that will be
    /// installed in the container. Normalization is performed by
    /// `prepare_context` before this field is stored, so callers
    /// (notably `build_config_labels`) can rely on the order and
    /// uniqueness without re-normalizing. `lang_display` is the
    /// separate human-readable form shown in startup logs.
    pub(super) lang_names: Vec<String>,
    /// Fingerprint of the template `setup_commands` that will run
    /// at container creation. Empty string when no template was
    /// selected or the template declared no setup_commands.
    /// Persisted in the `vibepod.template_setup_hash` label so the
    /// Phase-4.7 reuse gate can detect when a template's setup
    /// sequence has changed and force `--new` (setup only runs at
    /// creation, so we cannot retrofit new commands onto an existing
    /// container).
    pub(super) template_setup_hash: String,
    pub(super) store: SessionStore,
    pub(super) deferred_session: crate::session::Session,
    pub(super) extra_mounts: Vec<(String, String)>,
    /// 既存コンテナの状態（prepare.rs で検出）
    pub(super) container_status: ContainerStatus,
    /// ワークツリーモード：実行後にコンテナを削除する
    pub(super) is_disposable: bool,
    /// ネットワーク無効フラグ（ラベル生成に使用）
    pub(super) no_network: bool,
    /// ストリーム途絶タイムアウト（秒）。0 = 無効
    pub(super) prompt_idle_timeout: u64,
}

/// 環境変数のリストを正規化してハッシュ化する（値の変更も検知するため）。
/// ラベルに値を直接保存しないよう、16 桁の hex ハッシュのみを返す。
pub(super) fn hash_env_vars(env_vars: &[String]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut sorted = env_vars.to_vec();
    sorted.sort();
    let combined = sorted.join("\n");
    let mut hasher = DefaultHasher::new();
    combined.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn parse_mount_arg(arg: &str) -> anyhow::Result<(String, String)> {
    if let Some((host, container)) = arg.split_once(':') {
        Ok((host.to_string(), container.to_string()))
    } else {
        let path = std::path::Path::new(arg);
        let filename = path
            .file_name()
            .context("Invalid mount path")?
            .to_string_lossy();
        Ok((arg.to_string(), format!("/mnt/{}", filename)))
    }
}

pub fn detect_languages(workspace: &std::path::Path) -> Vec<(String, &'static str)> {
    let mut langs = Vec::new();
    if workspace.join("Cargo.toml").exists() {
        langs.push(("rust".to_string(), "Cargo.toml"));
    }
    if workspace.join("package.json").exists() {
        langs.push(("node".to_string(), "package.json"));
    }
    if workspace.join("go.mod").exists() {
        langs.push(("go".to_string(), "go.mod"));
    }
    if workspace.join("pyproject.toml").exists() {
        langs.push(("python".to_string(), "pyproject.toml"));
    } else if workspace.join("requirements.txt").exists() {
        langs.push(("python".to_string(), "requirements.txt"));
    }
    if workspace.join("pom.xml").exists() {
        langs.push(("java".to_string(), "pom.xml"));
    } else if workspace.join("build.gradle").exists() {
        langs.push(("java".to_string(), "build.gradle"));
    } else if workspace.join("build.gradle.kts").exists() {
        langs.push(("java".to_string(), "build.gradle.kts"));
    }
    langs
}

/// Single source of truth for language identifiers vibepod knows how
/// to install inside its container. `get_lang_install_cmd` matches on
/// these names, `is_supported_lang` checks membership, and error
/// messages that enumerate supported values read from this list so
/// they cannot drift out of sync.
pub const SUPPORTED_LANGS: &[&str] = &["rust", "node", "python", "go", "java"];

pub fn get_lang_install_cmd(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some("sudo apt-get update && sudo apt-get install -y build-essential && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && . $HOME/.cargo/env"),
        "node" => Some("curl -fsSL https://deb.nodesource.com/setup_22.x | sudo bash - && sudo apt-get install -y nodejs"),
        "python" => Some("sudo apt-get update && sudo apt-get install -y python3 python3-pip python3-venv"),
        "go" => Some("ARCH=$(uname -m) && GOARCH=$([ \"$ARCH\" = \"aarch64\" ] && echo arm64 || echo amd64) && curl -fsSL https://go.dev/dl/go1.24.2.linux-${GOARCH}.tar.gz | sudo tar -C /usr/local -xzf - && sudo sh -c 'echo \"export PATH=/usr/local/go/bin:\\$PATH\" > /etc/profile.d/go.sh'"),
        "java" => Some("sudo apt-get update && sudo apt-get install -y default-jdk"),
        _ => None,
    }
}

/// Return `true` iff `lang` has a known install command. Used by
/// template metadata parsing to reject `required_langs` values that
/// cannot actually be installed (a typo like "rsut" or an unsupported
/// runtime), instead of silently dropping them at install time.
pub fn is_supported_lang(lang: &str) -> bool {
    get_lang_install_cmd(lang).is_some()
}

pub fn validate_slack_channel_id(id: &str) -> bool {
    (id.starts_with('C') || id.starts_with('G')) && id.len() >= 9
}

/// コンテナ内 Claude Code が `$HOME/.claude/plugins` として読むデフォルトパス。
const DEFAULT_PLUGINS_CONTAINER_PATH: &str = "/home/vibepod/.claude/plugins";

/// ラベル中で「サニタイズ済み settings.json が有効」であることを示すマーカー。
/// 形式が `host:container` の通常マウント表現と衝突しないように
/// 専用 prefix を付けている。
pub(super) const SANITIZED_SETTINGS_LABEL_MARKER: &str =
    "sanitized_settings=/home/vibepod/.claude/settings.json";

/// `~/.claude/` 配下のグローバル設定ファイル・ディレクトリのマウント定義を構築する。
/// 存在するもののみ含まれる。read-only でマウントされる。
///
/// `plugins/` は特殊で、2 つのマウント先を返す:
/// 1. `/home/vibepod/.claude/plugins` — Claude Code が $HOME 経由で読む先
/// 2. `<host_home>/.claude/plugins` — `installed_plugins.json` 内の `installPath`
///    フィールドがホスト絶対パスを持つため、同じ絶対パスに再マウントして解決する
pub fn build_claude_config_mounts(home: &std::path::Path) -> Vec<(String, String)> {
    let claude_dir = home.join(".claude");
    let mut mounts = Vec::new();

    for (path, entry) in host_claude_stage_entries(&claude_dir) {
        mounts.push((
            path.to_string_lossy().to_string(),
            format!("/home/vibepod/.claude/{}", entry),
        ));
    }

    let plugins_dir = claude_dir.join("plugins");
    if plugins_dir.is_dir() {
        mounts.extend(plugins_mount_entries(&plugins_dir.to_string_lossy(), home));
    }

    mounts
}

/// ホストの `~/.claude/` からコンテナへ持ち込んでよい資産の **allowlist**。
/// `(entry_name, is_dir)` の組で、ここに列挙したものだけがコンテナに渡る。
///
/// deny list ではなく allowlist にしているのは意図的である。Claude Code が
/// 将来 `~/.claude/` 配下に新しい実行履歴ディレクトリを追加しても、
/// allowlist なら自動的に除外され続ける（deny list は追従漏れで漏洩する）。
///
/// **意図的に除外しているもの**:
/// - `sessions/` `projects/` `history.jsonl` `backups/` `file-history/`
///   `shell-snapshots/` `todos/` — 実行履歴・セッションデータ。サイズが
///   大きく、他プロジェクトの会話内容という機微情報を含むため。
/// - `settings.json` — host mode では `prepare_sanitized_settings_mount` が
///   hooks/statusLine を除去した別経路で扱う。template mode では
///   template 側の `permissions.deny`（review モードで `Edit(*)` /
///   `Write(*)` 等を封じる安全性の根拠）がホスト設定で上書きされては
///   ならないため、そもそも持ち込まない。
/// - `plugins/` — コンテナ側 2 箇所へのマウントが必要でファイルコピーでは
///   表現できないため、`plugins_mount_entries` が別途処理する。
pub const HOST_CLAUDE_ALLOWLIST: &[(&str, bool)] = &[
    ("CLAUDE.md", false),
    ("agents", true),
    ("skills", true),
    ("specs", true),
];

/// `claude_dir`（= `<home>/.claude`）配下で、allowlist に載っていて
/// かつ実際に存在するエントリを `(絶対パス, エントリ名)` で返す。
///
/// 存在しないものは黙ってスキップする（ホスト環境に `specs/` が無いのは
/// 異常ではなく通常であり、エラーにする理由がない）。
pub fn host_claude_stage_entries(
    claude_dir: &std::path::Path,
) -> Vec<(std::path::PathBuf, &'static str)> {
    let mut entries = Vec::new();
    for (name, is_dir) in HOST_CLAUDE_ALLOWLIST {
        let path = claude_dir.join(name);
        let exists = if *is_dir {
            path.is_dir()
        } else {
            path.is_file()
        };
        if exists {
            entries.push((path, *name));
        }
    }
    entries
}

/// template mode で、ホストの `~/.claude/plugins` をマウント集合に合流させる。
///
/// **優先順位: template 側が勝つ。** template が `plugins/` を持つ場合、
/// その mount が既に `/home/vibepod/.claude/plugins` を占有しているので
/// ホスト側は一切持ち込まない（docker は同一マウント先の重複を拒否するし、
/// そもそも template が plugin セットを規定する意図で置いている）。
///
/// ホスト plugins を staging へ**コピーしない**理由:
/// `installed_plugins.json` の `installPath` はホスト絶対パスを持つため、
/// staging 経由で単一マウントすると container 内で解決できず
/// `validate_template_installed_plugins` が hard fail する。host mode と
/// 同じ 2 重マウント（`plugins_mount_entries`）でホスト絶対パスを
/// container に投影するのが唯一整合する方法である。
pub fn merge_host_plugins_mounts(
    template_mounts: &mut Vec<(String, String)>,
    host_plugins: Option<&str>,
    home: &std::path::Path,
) {
    let Some(host_plugins) = host_plugins else {
        return;
    };
    let template_owns_plugins = template_mounts
        .iter()
        .any(|(_, container)| container == DEFAULT_PLUGINS_CONTAINER_PATH);
    if template_owns_plugins {
        return;
    }
    template_mounts.extend(plugins_mount_entries(host_plugins, home));
}

/// plugins ディレクトリに対応する 2 重マウントエントリを返す（ファイル存在チェック
/// は呼び出し側の責務）。
///
/// ホスト HOME が `/home/vibepod` の場合、(1) と (2) のコンテナ側パスが一致する
/// ため (2) を追加せず 1 本だけ返す（docker run -v が同一マウント先を拒否する）。
pub fn plugins_mount_entries(plugins_host: &str, home: &std::path::Path) -> Vec<(String, String)> {
    let mut entries = Vec::with_capacity(2);
    // (1) Claude Code が $HOME/.claude/plugins として読む先
    entries.push((
        plugins_host.to_string(),
        DEFAULT_PLUGINS_CONTAINER_PATH.to_string(),
    ));
    // (2) installed_plugins.json の installPath フィールドはホスト絶対パスを
    //     保持しているため、同じ絶対パスに再マウントして解決する。
    //     ただし `home` がコンテナ側 HOME `/home/vibepod` と一致する場合は
    //     (1) と重複するため追加しない。
    let absolute_container = home.join(".claude").join("plugins");
    if absolute_container != std::path::Path::new(DEFAULT_PLUGINS_CONTAINER_PATH) {
        entries.push((
            plugins_host.to_string(),
            absolute_container.to_string_lossy().to_string(),
        ));
    }
    entries
}

/// ホストの `~/.claude/settings.json` を読み、コンテナに持ち込めない
/// ホスト固有フィールドを除去した JSON 文字列を返す。
///
/// 除去対象:
/// - `hooks` — 絶対パスでホストスクリプトを参照するため
/// - `statusLine` — 同様にホストスクリプトを参照する可能性があるため
///
/// その他のフィールド（`env`, `permissions`, `enabledPlugins`,
/// `extraKnownMarketplaces`, `teammateMode` 等）はそのまま保持する。
pub fn sanitize_settings_json(input: &str) -> anyhow::Result<String> {
    let mut value: serde_json::Value =
        serde_json::from_str(input).context("Failed to parse settings.json")?;

    if let Some(obj) = value.as_object_mut() {
        obj.remove("hooks");
        obj.remove("statusLine");
    }

    serde_json::to_string_pretty(&value).context("Failed to serialize sanitized settings.json")
}

/// ホストの `~/.claude/settings.json` をサニタイズしたコピーを生成し、
/// コンテナにマウントするためのマウントエントリを返す。
///
/// サニタイズ済み JSON は `<config_dir>/runtime/<container_name>/settings.json`
/// に書き出される。この場所は vibepod が書き込み許可を持つ唯一の場所である。
///
/// ホスト側の `settings.json` が存在しない場合は `None` を返す（マウント追加不要）。
pub fn prepare_sanitized_settings_mount(
    home: &std::path::Path,
    config_dir: &std::path::Path,
    container_name: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let host_settings = home.join(".claude").join("settings.json");
    if !host_settings.is_file() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&host_settings)
        .with_context(|| format!("Failed to read {}", host_settings.display()))?;
    let sanitized = sanitize_settings_json(&raw)?;

    let runtime_dir = config_dir.join("runtime").join(container_name);
    std::fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("Failed to create {}", runtime_dir.display()))?;

    let target = runtime_dir.join("settings.json");
    std::fs::write(&target, sanitized)
        .with_context(|| format!("Failed to write {}", target.display()))?;

    // サニタイズ済みファイルにはホスト設定値（env、permissions 等）が含まれうるため、
    // token.json と同様に Unix では所有者のみ読み書き可能に制限する。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&target)
            .with_context(|| format!("Failed to read metadata of {}", target.display()))?
            .permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&target, perms)
            .with_context(|| format!("Failed to set permissions on {}", target.display()))?;
    }

    Ok(Some((
        target.to_string_lossy().to_string(),
        "/home/vibepod/.claude/settings.json".to_string(),
    )))
}

pub(super) fn build_container_config(
    ctx: &RunContext,
    image: String,
    no_network: bool,
) -> ContainerConfig {
    let gitconfig = ctx.home.join(".gitconfig");
    ContainerConfig {
        image,
        container_name: ctx.container_name.clone(),
        workspace_path: ctx.effective_workspace.clone(),
        claude_json: ctx
            .temp_claude_json
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        gitconfig: if gitconfig.exists() {
            Some(gitconfig.to_string_lossy().to_string())
        } else {
            None
        },
        env_vars: ctx.resolved_env_vars.clone(),
        network_disabled: no_network,
        extra_mounts: ctx.extra_mounts.clone(),
        labels: build_config_labels(ctx),
    }
}

/// コンテナのラベルを生成する（設定変更の検知に使用）。
pub(super) fn build_config_labels(ctx: &RunContext) -> std::collections::HashMap<String, String> {
    let mut labels = std::collections::HashMap::new();

    // マウントパスをソートして結合
    let mut mount_parts: Vec<String> = ctx
        .extra_mounts
        .iter()
        .map(|(h, c)| format!("{}:{}", h, c))
        .collect();
    mount_parts.sort();
    labels.insert("vibepod.mounts".to_string(), mount_parts.join("|"));

    labels.insert("vibepod.network".to_string(), ctx.no_network.to_string());

    // lang: persist the FULL set of languages the container was
    // provisioned with. `ctx.lang_names` is already sorted and
    // deduped (invariant established by `prepare_context`), so this
    // is a direct join. Using lang_display's first token would lose
    // template-added langs (e.g. "python (detected) + rust (template)"
    // → "python") and break the reuse check that verifies every
    // template-required lang is present.
    labels.insert("vibepod.lang".to_string(), ctx.lang_names.join(","));

    // Label schema version.
    //
    // - Missing / "1": pre-Phase-4.6, `vibepod.lang` may be in the
    //   legacy single-token format.
    // - "2": Phase 4.6 — `vibepod.lang` stores the full comma-joined
    //   lang set. Template `required_langs` hard-fail gate is enabled.
    // - "3": Phase 4.7 — adds `vibepod.template_setup_hash`. Template
    //   `setup_commands` hash gate is enabled. Containers labeled with
    //   version < 3 fall back to warnings on setup_commands drift
    //   because their stored hash is absent and cannot be trusted.
    labels.insert("vibepod.labels_version".to_string(), "3".to_string());

    // Template setup_commands fingerprint. Empty string when the
    // container was not created with a template, or the template
    // declared no setup_commands. Updating a template's setup_commands
    // mutates this hash; the reuse gate then forces the user to
    // `--new` because setup_cmd only runs at container creation.
    labels.insert(
        "vibepod.template_setup_hash".to_string(),
        ctx.template_setup_hash.clone(),
    );

    // ワークスペースパスを保存（ps コマンドでの表示に使用）
    labels.insert(
        "vibepod.workspace".to_string(),
        ctx.effective_workspace.clone(),
    );

    // ユーザー環境変数のハッシュを保存（--env 値の変更を検知するため値もハッシュ化）
    // セキュリティ上の理由でラベルに値を直接保存せず、ハッシュのみ格納する
    let env_hash = hash_env_vars(&ctx.resolved_env_vars);
    labels.insert("vibepod.env_hash".to_string(), env_hash);

    labels
}

pub async fn execute(opts: RunOptions) -> Result<()> {
    let interactive = !opts.resume && opts.prompt.is_none();

    let Some(ctx) = prepare::prepare_context(&opts).await? else {
        return Ok(());
    };

    // 排他チェック: prompt.lock が有効なら（= --prompt セッション実行中）全モードで拒否
    let vibepod_dir = std::path::PathBuf::from(&ctx.effective_workspace).join(".vibepod");
    if let Some(pid) = lock::PromptLock::check(&vibepod_dir) {
        anyhow::bail!(
            "セッション実行中です (PID: {})\n停止するには: vibepod stop",
            pid
        );
    }

    // 停止中コンテナでは claude プロセスは存在し得ないのでスキップする
    if !interactive && ctx.container_status == ContainerStatus::Running {
        let runtime = crate::runtime::DockerRuntime::new().await?;
        let has_running_session = runtime
            .has_claude_process(&ctx.container_name)
            .await
            .with_context(|| {
                format!(
                    "実行中セッションの確認に失敗しました (container: {})",
                    ctx.container_name
                )
            })?;
        if has_running_session {
            anyhow::bail!("セッション実行中です\n停止するには: vibepod stop");
        }
    }

    if interactive {
        interactive::run_interactive(&opts, &ctx).await
    } else {
        prompt::run_fire_and_forget(&opts, &ctx).await
    }
}
