use anyhow::{Context, Result};

use crate::config;
use crate::runtime::{ContainerConfig, ContainerStatus};
use crate::session::SessionStore;

mod interactive;
pub mod lock;
pub mod prepare;
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
    /// コンテナ内 Claude Code の更新チェック方針（`--update` / `--no-update`）。
    pub update_policy: crate::update::UpdatePolicy,
    /// `--model <name>`: そのまま `claude --model <name>` に渡すモデル名。
    /// vibepod は値を検証しない（正当性は claude 側が判定する）。`None` の
    /// ときは何も渡さず claude のデフォルト解決に任せる。
    pub model: Option<String>,
    /// `--no-auto-build`: イメージが無いときの自動ビルドを抑止する。
    pub no_auto_build: bool,
    /// `--timeout <duration>`: `--prompt` 実行の実時間上限（生文字列）。
    /// `None` のときは `DEFAULT_OVERALL_TIMEOUT_SECS` を使う。パースは
    /// `parse_timeout_secs` が担う（`0` で無効化）。
    pub timeout: Option<String>,
    /// `--verbose`: `--prompt` 実行で per-event の整形ログを stdout に流す
    /// （1.7 未満の挙動）。既定は要約のみ。`logs.txt` への保存は常に継続する。
    pub verbose: bool,
}

/// `--prompt` セッションの実時間上限のデフォルト（秒）。
///
/// 30 分。大きめのタスク・数回のリトライ・一時的な利用上限バックオフを
/// 吸収できる程度に長く、かつ暴走（無限リトライや長時間バックオフ）で
/// コンテナが何時間も居座るのを防げる程度に短い、という妥協点。
/// `--timeout` で上書きでき、`--timeout 0` で無効化できる。
pub const DEFAULT_OVERALL_TIMEOUT_SECS: u64 = 30 * 60;

/// `--timeout` の値を秒数に解釈する純関数。
///
/// 受理する形式:
/// - 裸の整数（秒）: `"1800"` → 1800。`"0"` は「無効化」を意味する 0。
/// - サフィックス付き duration: `"30m"` / `"1h30m"` / `"90s"` など
///   （`config::parse_duration_to_seconds` に委譲）。
///
/// 外部状態に依存しないためユニットテストで網羅できる。不正入力は
/// 運用者が直せるよう、受理形式を添えた明確なエラーを返す。
pub fn parse_timeout_secs(raw: &str) -> anyhow::Result<u64> {
    let s = raw.trim();
    if s.is_empty() {
        anyhow::bail!("--timeout must not be empty (use seconds like 1800, a duration like 30m, or 0 to disable)");
    }
    // 裸の整数は秒として解釈する（"0" を含む）。duration パーサは末尾に
    // 単位を要求するため、ここで先に整数を処理する。
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }
    crate::config::parse_duration_to_seconds(s).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid --timeout '{}': use bare seconds (e.g. 1800), a duration like 30m / 1h30m, or 0 to disable",
            raw
        )
    })
}

/// `--prompt` 実行後に呼び出し元が読む簡潔な要約を組み立てる純関数。
///
/// 生の stream-json を垂れ流す代わりに、終了ステータス・結果本文・変更
/// ファイル一覧・`logs.txt` のフルパスだけを 1 ブロックにまとめる。表示
/// 層のロジックを I/O から切り離してユニットテスト可能にするため、必要な
/// 値はすべて引数で受け取る。
pub fn render_run_summary(
    success: bool,
    reason: &str,
    result_text: Option<&str>,
    changed_files: &crate::git::ChangedFiles,
    logs_path: &str,
) -> String {
    let mut out = String::from("Summary:\n");
    if success {
        out.push_str("  Status: success\n");
    } else {
        out.push_str(&format!("  Status: failed ({})\n", reason));
    }

    if let Some(text) = result_text {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            out.push_str("  Result: ");
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    // 「本当に無変更 (none)」と「算出できなかった (unavailable)」を必ず
    // 区別する。潰すと、算出失敗が「変更なし」に見えて呼び出し元が
    // 誤判断する（指摘 #2）。
    match changed_files {
        crate::git::ChangedFiles::Unavailable => {
            out.push_str("  Changed files: (could not be computed — see full logs)\n");
        }
        crate::git::ChangedFiles::Computed(files) if files.is_empty() => {
            out.push_str("  Changed files: (none)\n");
        }
        crate::git::ChangedFiles::Computed(files) => {
            out.push_str(&format!("  Changed files ({}):\n", files.len()));
            for f in files {
                out.push_str("    ");
                out.push_str(f);
                out.push('\n');
            }
        }
    }

    out.push_str("  Full logs: ");
    out.push_str(logs_path);
    out
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
    /// `~/.codex/` の allowlist(auth.json / config.toml)をコピーした
    /// per-container ディレクトリ(`<runtime_dir>/codex/`)。存在しない場合
    /// (auth.json 欠如)は `None` — コンテナには codex 認証を注入しない。
    pub(super) codex_dir: Option<std::path::PathBuf>,
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
    /// `--prompt` 実行の実時間上限（秒）。0 = 無効。`--timeout` 未指定時は
    /// `DEFAULT_OVERALL_TIMEOUT_SECS`。prepare_context でパース済みの値。
    pub(super) overall_timeout: u64,
    /// `--verbose`: per-event の整形ログを stdout に流すか。既定 false（要約のみ）。
    pub(super) verbose: bool,
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

/// コンテナ内 Claude Code が `$HOME/.claude/plugins` として読むデフォルトパス。
const DEFAULT_PLUGINS_CONTAINER_PATH: &str = "/home/vibepod/.claude/plugins";

/// ラベル中で「サニタイズ済み settings.json が有効」であることを示すマーカー。
/// 形式が `host:container` の通常マウント表現と衝突しないように
/// 専用 prefix を付けている。
pub(super) const SANITIZED_SETTINGS_LABEL_MARKER: &str =
    "sanitized_settings=/home/vibepod/.claude/settings.json";

/// ラベル中で「`/home/vibepod/.codex` が bind mount されている」ことを示す
/// マーカー。bind mount はコンテナ作成時に固定されるため、codex の有無を
/// mounts ラベルの構成要素に含めないと、後から `~/.codex/auth.json` が
/// 追加/削除されても既存コンテナとの構成差分として検出されない
/// (round 2 で見つかった不具合)。形式が `host:container` の通常マウント
/// 表現と衝突しないように専用 prefix を付けている
/// （`SANITIZED_SETTINGS_LABEL_MARKER` と同じパターン）。
pub(super) const CODEX_MOUNT_LABEL_MARKER: &str = "codex=/home/vibepod/.codex";

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
/// - `settings.json` — `prepare_sanitized_settings_mount` が hooks/statusLine を
///   除去した別経路で扱うため、この allowlist には含めない。
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
///
/// **symlink の扱い**:
///
/// - **top-level エントリ自体**（`~/.claude/skills` そのものが symlink 等）は
///   `is_dir()` / `is_file()` が symlink を追従するため、**意図的に追従を
///   許容する**。dotfiles 管理でディレクトリごと symlink にする構成は一般的で、
///   ユーザー自身の明示的な資産配置とみなせるため。追従先の実ディレクトリが
///   そのまま ro バインドマウントされる。
/// - そのディレクトリ**配下**の symlink については、返した実パスを docker が
///   read-only でバインドマウントするだけで、vibepod 側でコピー・追従は
///   行わない。配下の symlink はマウント境界の中の symlink ファイルとして
///   そのままコンテナに現れ、コンテナ内で解決される。マウント範囲外
///   （ホストの `~/.claude/` 外）を指す symlink はコンテナのファイルシステム
///   名前空間では対象へ到達できず解決不能になるため、外部の巨大ツリーや
///   秘密がコンテナへ流入することはない。
pub fn host_claude_stage_entries(
    claude_dir: &std::path::Path,
) -> Vec<(std::path::PathBuf, &'static str)> {
    let mut entries = Vec::new();
    for (name, is_dir) in HOST_CLAUDE_ALLOWLIST {
        let path = claude_dir.join(name);
        // is_dir()/is_file() follow symlinks: a top-level symlinked entry
        // (a whole ~/.claude/skills symlinked elsewhere) is followed on
        // purpose. The resolved real path is then bind-mounted read-only.
        // Symlinks nested inside are left as-is: docker ro-mounts the
        // directory, and any symlink pointing outside the mounted path
        // simply cannot resolve inside the container's filesystem namespace.
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

/// ホストの `~/.codex/` 配下でコンテナに持ち込んでよい資産の **allowlist**。
/// `auth.json` と `config.toml` の 2 つのみ。
///
/// **意図的に除外しているもの**: `history.jsonl` / `goals_*.sqlite` / `cache/` 等。
/// 機微・不要データであり、codex レビューの実行に一切必要ないため。
pub const HOST_CODEX_ALLOWLIST: &[&str] = &["auth.json", "config.toml"];

/// `codex_dir`(= `<home>/.codex`)配下で、allowlist に載っていてかつ実際に
/// 存在するファイルを `(絶対パス, ファイル名)` で返す。存在しないものは
/// 黙ってスキップする(`config.toml` が無いホストは異常ではなく通常の運用)。
pub fn host_codex_stage_entries(
    codex_dir: &std::path::Path,
) -> Vec<(std::path::PathBuf, &'static str)> {
    let mut entries = Vec::new();
    for name in HOST_CODEX_ALLOWLIST {
        let path = codex_dir.join(name);
        if path.is_file() {
            entries.push((path, *name));
        }
    }
    entries
}

/// `dir` 配下から、`HOST_CODEX_ALLOWLIST` のうち `keep` に含まれない名前の
/// ファイルを削除する(存在すれば削除、無ければ何もしない)。
///
/// `prepare_codex_mount` の P1(auth 消失時に残置を全消去)・P2(config.toml
/// のみ消失時に差分だけ消去)双方が使う共通のリコンサイル処理。`dir` 自体が
/// まだ存在しない場合(初回 run で `create_dir_all` 前に呼ばれるケース)は
/// 削除対象が無いので何もしない。
///
/// 削除失敗は `unwrap`/`expect` で握りつぶさず、どのパスの削除に失敗したかを
/// context に含めて呼び出し元へ伝播する。
fn reconcile_codex_stage_dir(dir: &std::path::Path, keep: &[&str]) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for name in HOST_CODEX_ALLOWLIST {
        if keep.contains(name) {
            continue;
        }
        let path = dir.join(name);
        if path.is_file() {
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to remove stale {}", path.display()))?;
        }
    }
    Ok(())
}

/// ステージ済み `auth.json` を保持すべきか(= ホスト側のコピーで上書きしない)を
/// 判定する純関数。
///
/// コンテナ内 codex はステージ先(rw マウント経由)の `auth.json` をトークン
/// リフレッシュ時に書き換える。次回 `vibepod run` がこれを無条件にホスト側の
/// (リフレッシュ前の)`auth.json` で上書きすると、ローテーションされたリフレッシュ
/// トークンが失われ、以後コンテナ内 codex が認証不能になる(codex レビュー round 3
/// P1)。
///
/// ステージ済みファイルの mtime がホスト側より **厳密に新しい** 場合のみ「コンテナが
/// 更新した」とみなして保持する(= コピーしない)。mtime が同じ、またはホスト側の
/// mtime が新しい(= ユーザーが再ログインした等)場合は、従来どおりホスト側優先で
/// 上書きする。ホストへの書き戻しは行わない(「ホスト原本に触れない」原則のため)。
pub fn should_keep_staged_auth(
    host_mtime: std::time::SystemTime,
    staged_mtime: std::time::SystemTime,
) -> bool {
    staged_mtime > host_mtime
}

/// ホストの `~/.codex/auth.json`(と存在すれば `config.toml`)を
/// `<config_dir>/runtime/<container_name>/codex/` にコピーし、そのディレクトリの
/// パスを返す。呼び出し元はこのパスをコンテナへ `/home/vibepod/.codex` として
/// **rw** マウントする(codex がトークンリフレッシュ時に auth.json を書き換える
/// ため。コピーなのでホスト原本には影響しない — `.claude.json` と同じパターン)。
///
/// `auth.json` が存在しない場合は `None` を返し、codex 注入をスキップする
/// (vibepod 自体は動作継続するが、コンテナ内で codex レビューは使えない)。
/// エラーの握りつぶし禁止のルールに従い、この場合は stderr に理由を明示する。
///
/// `config.toml` のみ無く `auth.json` はある場合は、警告なしで auth.json だけ
/// コピーして続行する(`config.toml` 省略は codex のデフォルト設定を使う正常な
/// 運用パターンのため)。
///
/// **`auth.json` は「新しい方を保持」する**(codex レビュー round 3 P1): ステージ済み
/// `auth.json` の mtime がホスト側より新しければ、コンテナ内 codex がリフレッシュした
/// ものとみなしコピーをスキップする(`should_keep_staged_auth` 参照)。`config.toml` は
/// 認証情報ではなく設定ファイルのため、この特例の対象外であり常にホスト側で上書きする。
pub fn prepare_codex_mount(
    home: &std::path::Path,
    config_dir: &std::path::Path,
    container_name: &str,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    let host_codex_dir = home.join(".codex");
    let entries = host_codex_stage_entries(&host_codex_dir);

    let runtime_codex_dir = config_dir
        .join("runtime")
        .join(container_name)
        .join("codex");

    let has_auth = entries.iter().any(|(_, name)| *name == "auth.json");
    if !has_auth {
        // P1: ホストの auth.json が無い(未認証 or 取り消し済み)。過去の run で
        // ステージ済みの認証情報が残っていると、既存コンテナの bind mount
        // 経由で使われ続けてしまうため、ディレクトリ自体は残したまま中身だけ
        // 全消去する(keep が空 = allowlist 全ファイルが削除対象)。
        reconcile_codex_stage_dir(&runtime_codex_dir, &[]).with_context(|| {
            format!(
                "Failed to clear stale codex assets in {}",
                runtime_codex_dir.display()
            )
        })?;
        eprintln!(
            "codex auth not found (~/.codex/auth.json); codex review is unavailable in this container"
        );
        return Ok(None);
    }

    std::fs::create_dir_all(&runtime_codex_dir)
        .with_context(|| format!("Failed to create {}", runtime_codex_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime_codex_dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| {
                format!(
                    "Failed to set permissions on {}",
                    runtime_codex_dir.display()
                )
            })?;
    }

    // P2: 今回の entries に無い allowlist ファイル(例: ホストで config.toml が
    // 削除された)がステージに残っていると無期限に使われ続けるため、コピー前に
    // 差分を削除しておく。
    let keep_names: Vec<&str> = entries.iter().map(|(_, name)| *name).collect();
    reconcile_codex_stage_dir(&runtime_codex_dir, &keep_names).with_context(|| {
        format!(
            "Failed to reconcile stale codex assets in {}",
            runtime_codex_dir.display()
        )
    })?;

    for (src, name) in &entries {
        let dst = runtime_codex_dir.join(name);

        if *name == "auth.json" && dst.is_file() {
            let staged_mtime = std::fs::metadata(&dst)
                .and_then(|m| m.modified())
                .with_context(|| format!("Failed to read mtime of {}", dst.display()))?;
            let host_mtime = std::fs::metadata(src)
                .and_then(|m| m.modified())
                .with_context(|| format!("Failed to read mtime of {}", src.display()))?;

            if should_keep_staged_auth(host_mtime, staged_mtime) {
                // コンテナ内 codex がトークンリフレッシュ済みの auth.json を、
                // 古いホストコピーで上書きしない(round 3 P1)。パーミッションは
                // 初回コピー時に 0600 済みなのでそのまま維持する。
                continue;
            }
        }

        std::fs::copy(src, &dst)
            .with_context(|| format!("Failed to copy {} to {}", src.display(), dst.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Failed to set permissions on {}", dst.display()))?;
        }
    }

    Ok(Some(runtime_codex_dir))
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
        codex_dir: ctx
            .codex_dir
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

/// mounts ラベル文字列を構築する。`base_parts` には通常マウント
/// （host:container 形式）や sanitized_settings マーカー等を呼び出し側で
/// 積んだ状態で渡す。ここでは codex マウントの有無を追加してから
/// ソート・結合するところまでを一箇所に集約し、build_config_labels
/// （コンテナ作成時にラベルへ書き込む値）と prepare_context の事前差分
/// チェック（9b、既存コンテナと比較する現在値）の両方が全く同じロジックで
/// mounts ラベルを組み立てるようにする。二箇所が独立に実装され食い違う
/// ことが round 2 で見つかった不具合の根本原因だったため。
pub(super) fn build_mounts_label(mut base_parts: Vec<String>, codex_present: bool) -> String {
    if codex_present {
        base_parts.push(CODEX_MOUNT_LABEL_MARKER.to_string());
    }
    base_parts.sort();
    base_parts.join("|")
}

/// コンテナのラベルを生成する（設定変更の検知に使用）。
pub(super) fn build_config_labels(ctx: &RunContext) -> std::collections::HashMap<String, String> {
    let mut labels = std::collections::HashMap::new();

    // マウントパスをソートして結合
    let mount_parts: Vec<String> = ctx
        .extra_mounts
        .iter()
        .map(|(h, c)| format!("{}:{}", h, c))
        .collect();
    labels.insert(
        "vibepod.mounts".to_string(),
        build_mounts_label(mount_parts, ctx.codex_dir.is_some()),
    );

    labels.insert("vibepod.network".to_string(), ctx.no_network.to_string());

    // lang: persist the FULL set of languages the container was
    // provisioned with. `ctx.lang_names` is already sorted and
    // deduped (invariant established by `prepare_context`), so this
    // is a direct join. Storing the whole set (not lang_display's first
    // token) keeps the reuse check honest when multiple languages are
    // installed.
    labels.insert("vibepod.lang".to_string(), ctx.lang_names.join(","));

    // Label schema version. Monotonically increasing: never decrement,
    // even when features are removed (a smaller value would misidentify
    // newer containers as older ones). Previous releases used "1" (legacy
    // single-token `vibepod.lang`), "2" (full comma-joined lang set), and
    // "3" (added the now-removed `vibepod.template_setup_hash`). Removing
    // the template machinery advances to "4".
    //
    // Currently this value is NOT read anywhere — no code branches on it.
    // It is written and reserved for a future backward-compatibility gate
    // that may need to distinguish container schema generations.
    labels.insert("vibepod.labels_version".to_string(), "4".to_string());

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_mounts_label_is_deterministic() {
        // codex マウントあり/なしそれぞれで、同じ入力なら常に同じ文字列に
        // なること(既存コンテナとの比較が安定する前提条件)。
        let base = vec![
            "/host/a:/container/a".to_string(),
            "/host/b:/container/b".to_string(),
        ];
        let with_codex_1 = build_mounts_label(base.clone(), true);
        let with_codex_2 = build_mounts_label(base.clone(), true);
        assert_eq!(with_codex_1, with_codex_2);

        let without_codex_1 = build_mounts_label(base.clone(), false);
        let without_codex_2 = build_mounts_label(base, false);
        assert_eq!(without_codex_1, without_codex_2);
    }

    #[test]
    fn test_build_mounts_label_detects_codex_presence_change() {
        // codex の有無が変わると mounts ラベルが変わり、
        // prepare.rs 側の warn_config_changes で構成差分として検出される。
        let base = vec!["/host/a:/container/a".to_string()];
        let with_codex = build_mounts_label(base.clone(), true);
        let without_codex = build_mounts_label(base, false);

        assert_ne!(with_codex, without_codex);
        assert!(
            with_codex.contains(CODEX_MOUNT_LABEL_MARKER),
            "expected codex marker in: {}",
            with_codex
        );
        assert!(
            !without_codex.contains(CODEX_MOUNT_LABEL_MARKER),
            "codex marker should be absent in: {}",
            without_codex
        );
    }

    #[test]
    fn test_build_mounts_label_stable_regardless_of_input_order() {
        // codex の有無が同じなら、base_parts の入力順が違っても(sort される
        // ため)同じ結果になる = 順序違いだけで誤った差分検知をしない。
        let base_order_1 = vec![
            "/host/a:/container/a".to_string(),
            "/host/b:/container/b".to_string(),
        ];
        let base_order_2 = vec![
            "/host/b:/container/b".to_string(),
            "/host/a:/container/a".to_string(),
        ];

        assert_eq!(
            build_mounts_label(base_order_1.clone(), true),
            build_mounts_label(base_order_2.clone(), true)
        );
        assert_eq!(
            build_mounts_label(base_order_1, false),
            build_mounts_label(base_order_2, false)
        );
    }
}
