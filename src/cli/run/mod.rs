use anyhow::{Context, Result};

use crate::config;
use crate::runtime::{ContainerConfig, ContainerStatus};
use crate::session::SessionStore;

mod interactive;
pub mod lock;
mod prepare;
mod prompt;

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
    pub(super) global_config: config::GlobalConfig,
    pub(super) home: std::path::PathBuf,
    pub(super) worktree_branch_name: Option<String>,
    pub(super) worktree_dir_name: Option<String>,
    pub(super) lang_display: String,
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

pub fn validate_slack_channel_id(id: &str) -> bool {
    (id.starts_with('C') || id.starts_with('G')) && id.len() >= 9
}

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

    let claude_md = claude_dir.join("CLAUDE.md");
    if claude_md.is_file() {
        mounts.push((
            claude_md.to_string_lossy().to_string(),
            "/home/vibepod/.claude/CLAUDE.md".to_string(),
        ));
    }

    let skills_dir = claude_dir.join("skills");
    if skills_dir.is_dir() {
        mounts.push((
            skills_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/skills".to_string(),
        ));
    }

    let agents_dir = claude_dir.join("agents");
    if agents_dir.is_dir() {
        mounts.push((
            agents_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/agents".to_string(),
        ));
    }

    let plugins_dir = claude_dir.join("plugins");
    if plugins_dir.is_dir() {
        let plugins_host = plugins_dir.to_string_lossy().to_string();
        // (1) Claude Code が $HOME/.claude/plugins として読む先
        mounts.push((
            plugins_host.clone(),
            "/home/vibepod/.claude/plugins".to_string(),
        ));
        // (2) installed_plugins.json の installPath フィールドはホスト絶対パスを
        //     保持しているため、同じ絶対パスに再マウントして解決する
        let absolute_container_path = format!("{}/.claude/plugins", home.to_string_lossy());
        mounts.push((plugins_host, absolute_container_path));
    }

    mounts
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

    // lang: setup_cmd がある場合は lang_display から推測するより、
    // lang_names を RunContext に保存する方がきれいだが、
    // ここでは lang_display の先頭部分を使う
    let lang_value = ctx
        .lang_display
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    labels.insert("vibepod.lang".to_string(), lang_value);

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

    // --prompt 開始時: interactive セッションが実行中かも確認
    if !interactive {
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
