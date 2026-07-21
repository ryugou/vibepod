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
    /// `~/.codex/` の allowlist(auth.json / config.toml)をコピーした、
    /// 全コンテナ共有のユーザー単位ステージディレクトリ(`<config_dir>/codex/`)。
    /// per-container ではないため、disposable 実行(`--new` / worktree)の
    /// `runtime_dir` 削除では消えない(round 4 で per-container 配置から移行)。
    /// 存在しない場合(auth.json 欠如)は `None` — コンテナには codex 認証を
    /// 注入しない。
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

/// `dir` 配下を実際に列挙し、`keep` に含まれない名前のエントリを
/// **ファイル・ディレクトリ・symlink 問わずすべて削除**する完全リコンサイル。
///
/// `prepare_codex_mount` の P1(auth 消失時に残置を全消去)・P2(config.toml
/// のみ消失時に差分だけ消去)双方が使う共通のリコンサイル処理。`dir` 自体が
/// まだ存在しない場合(初回 run で `create_dir_all` 前に呼ばれるケース)は
/// 削除対象が無いので何もしない。
///
/// 以前は `HOST_CODEX_ALLOWLIST` の名前だけをループして keep 外の名前を消す
/// 実装だったため、allowlist に載っていない名前(`history.jsonl` やコンテナが
/// 勝手に作った `cache/` 等)を一切見ずに素通りしていた。`.codex` は rw マウント
/// されているため、コンテナ内プロセスは任意のファイル・ディレクトリをステージに
/// 作成でき、それが allowlist をすり抜けて他コンテナからも参照可能な形で永続化
/// してしまう(codex レビュー round 5 P1-b)。`read_dir` で中身を実際に見て
/// keep 外を漏れなく消すことでこれを防ぐ。
///
/// 削除対象がディレクトリ(かつ symlink でない、`DirEntry::file_type()` は
/// symlink を辿らない)なら `remove_dir_all`、それ以外(ファイルまたは
/// symlink)なら `remove_file`(symlink はこれで辿らず unlink できる)を使う。
/// 削除ごとに「ファイル名のみ」を stderr に出す(機微データの無言蓄積を防ぐ
/// 運用可視性のため。内容は絶対に出さない)。
///
/// 削除失敗・列挙失敗は `unwrap`/`expect` で握りつぶさず、どのパスの操作に
/// 失敗したかを context に含めて呼び出し元へ伝播する。
fn reconcile_codex_stage_dir(dir: &std::path::Path, keep: &[&str]) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let read_dir = std::fs::read_dir(dir)
        .with_context(|| format!("Failed to list codex stage dir {}", dir.display()))?;

    for entry in read_dir {
        let entry = entry
            .with_context(|| format!("Failed to read a directory entry in {}", dir.display()))?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if keep.contains(&name_str.as_ref()) {
            continue;
        }

        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("Failed to determine file type of {}", path.display()))?;

        if file_type.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("Failed to remove stale directory {}", path.display()))?;
        } else {
            // ファイルまたは symlink。remove_file は symlink 自体を unlink し、
            // リンク先(ホスト任意パスの可能性がある)には触れない。
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to remove stale {}", path.display()))?;
        }
        eprintln!("removed stale codex stage entry not in allowlist: {name_str}");
    }
    Ok(())
}

/// `dst` を `symlink_metadata`(リンクを辿らない)で判定し、**通常ファイル
/// (regular file)以外はすべて削除**して stderr に警告を出す。
///
/// コンテナは rw マウント経由でステージ内の `auth.json` / `config.toml` を、
/// ホスト任意パスへの symlink だけでなく、ディレクトリ等の非通常ファイルにも
/// 差し替えられる(codex レビュー round 8 P1)。`reconcile_codex_stage_dir` は
/// allowlist に**含まれる名前**のエントリを file_type を一切見ずに素通りする
/// ため、名前だけ `auth.json` のディレクトリへの差し替えはそちらでは捕まらず、
/// この関数が唯一の防波堤になる。特にディレクトリへの差し替えを放置すると、
/// mtime が新しい `auth.json` ディレクトリが `should_keep_staged_auth` の
/// keep-newer 判定を通ってしまい、以後ホストからの再作成が恒久的にスキップ
/// されて認証が壊れたままになる。
///
/// `dst.is_file()` や `std::fs::metadata` はリンクを辿って `true` / リンク先の
/// 情報を返すため、これらで判定・処理すると mtime 比較やコピー先の解決が
/// ホスト任意ファイルを対象にしてしまう(コンテナ→ホストの書き込み境界の破れ、
/// codex レビュー round 5 P1-a)。呼び出し元は、この関数が戻った後は「`dst` は
/// 存在しないか、通常ファイル」ものとして扱ってよい。
///
/// `dst` が存在しない場合は何もしない(正常系)。削除対象がディレクトリなら
/// `remove_dir_all`、それ以外(symlink 等)なら `remove_file`(symlink はこれで
/// 辿らず unlink できる)を使う。`symlink_metadata` 自体の失敗(パーミッション
/// 等)および削除失敗は握りつぶさず伝播する。
fn remove_dst_if_not_regular_file(dst: &std::path::Path) -> anyhow::Result<()> {
    let meta = match std::fs::symlink_metadata(dst) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("Failed to stat {}", dst.display())),
    };

    if meta.file_type().is_file() {
        return Ok(());
    }

    if meta.file_type().is_dir() {
        std::fs::remove_dir_all(dst)
            .with_context(|| format!("Failed to remove staged directory at {}", dst.display()))?;
    } else {
        std::fs::remove_file(dst).with_context(|| {
            format!(
                "Failed to remove non-regular staged entry at {}",
                dst.display()
            )
        })?;
    }
    eprintln!(
        "staged codex file was not a regular file (symlink or directory); removed (possible \
         container tampering): {}",
        dst.display()
    );
    Ok(())
}

/// `dst` が改ざんされていない通常ファイルで、かつ `src` と完全に同一の
/// バイト列を持つかを判定するヘルパー(codex レビュー round 8 P2-2)。
/// `copy_codex_asset_atomically` が「コピー自体を省略してよいか」の事前
/// チェックに使う。
///
/// - `dst` が存在しない、または `symlink_metadata` 上 regular file でない
///   (symlink・ディレクトリ等)場合は `false`。`symlink_metadata` を使う
///   ため symlink を辿ることはない
/// - `dst` の読み取りに失敗した場合も `false` を返す。`dst` はステージ側で
///   改ざんされている可能性がある対象なので、読めなければ安全側(=コピー
///   実行)に倒すのが妥当
/// - `src` の読み取りに失敗した場合も `false` を返すが、これは「同一でない
///   と決め打ちしてコピーを握りつぶす」ものではない。呼び出し元は `false`
///   の場合そのまま通常のコピー処理へ進み、そこで `std::fs::copy(src, ..)`
///   が同じ理由で失敗して anyhow context 付きで自然に伝播される
fn staged_asset_matches_host(src: &std::path::Path, dst: &std::path::Path) -> bool {
    let is_regular_file = matches!(
        std::fs::symlink_metadata(dst),
        Ok(meta) if meta.file_type().is_file()
    );
    if !is_regular_file {
        return false;
    }

    let (Ok(src_bytes), Ok(dst_bytes)) = (std::fs::read(src), std::fs::read(dst)) else {
        return false;
    };
    src_bytes == dst_bytes
}

/// ホストの `src` をステージ内 `dst` へコピーする。
///
/// 旧実装は固定名の一時ファイル(`<name>.tmp`)へ書き込んでから rename する
/// 方式だったが、これはコンテナが rw マウント経由でステージを操作できる
/// 前提のもとで二重に危険だった(codex レビュー round 6):
///
/// - **P1(セキュリティ)**: コンテナが `<name>.tmp` という固定名自体を
///   ホスト任意パスへの symlink に差し替えておくと、次回 run の
///   `std::fs::copy(src, &tmp)` がそのリンクを辿り、`dst` 側の非通常ファイル
///   防御(`remove_dst_if_not_regular_file`)を迂回してホスト任意ファイルを
///   上書きできた。
/// - **P2(信頼性)**: 複数の `vibepod run` が同時にステージ準備を行うと
///   同じ固定名を取り合い、一方が rename した直後に他方の chmod/rename が
///   `NotFound` で失敗する不定期な競合が起きていた。
///
/// これを避けるため、同じディレクトリ内に `tempfile::NamedTempFile::new_in`
/// で予測不能な一意名のファイルを排他生成する。名前を事前に知りようがない
/// ため symlink 差し替えの標的になり得ず(P1 解消)、並行呼び出し間でも
/// 生成される名前が衝突しない(P2 解消)。
///
/// 一時ファイルへの権限設定(0600)は `persist`(rename 相当)**前**に行う
/// (persist は権限を保持するため、後に設定すると一瞬でも緩い権限の
/// ファイルが `dst` の場所に見える可能性がある)。`persist` 自体は rename と
/// 同じく symlink を辿らずディレクトリエントリを置き換えるため、置換時点で
/// `dst` が symlink であっても安全だが、改ざんの可能性を運用者が把握できる
/// よう persist 直前に `remove_dst_if_not_regular_file` で検査・警告する
/// (round 5 の防御をそのまま維持)。
///
/// 冒頭で `dst` の内容が `src` と既に同一かを確認し(P2-2、codex レビュー
/// round 8)、同一ならコピー・chmod・rename を一切行わずに早期リターンする。
/// 無駄な mtime 更新を避けられるほか、`should_keep_staged_auth` の
/// 「同値はステージ優先」判定(P2-1)は実際に値が変わった場合にのみ意味を
/// 持つため、内容が同じなら mtime を動かさない方が判定全体の安定性も上がる。
/// この早期リターンはパーミッションを検査しないため、コンテナがこの経路で
/// `dst` の権限だけ緩めていた場合の修復は呼び出し元
/// (`prepare_codex_mount` 内の `enforce_staged_permissions`)に委ねる
/// (round 9 P1)。
fn copy_codex_asset_atomically(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    if staged_asset_matches_host(src, dst) {
        return Ok(());
    }

    let stage_dir = dst.parent().with_context(|| {
        format!(
            "codex stage destination {} has no parent directory",
            dst.display()
        )
    })?;

    let tmp = tempfile::NamedTempFile::new_in(stage_dir).with_context(|| {
        format!(
            "Failed to create a unique temp file in {} for staging {}",
            stage_dir.display(),
            dst.display()
        )
    })?;

    std::fs::copy(src, tmp.path()).with_context(|| {
        format!(
            "Failed to copy {} to {}",
            src.display(),
            tmp.path().display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set permissions on {}", tmp.path().display()))?;
    }

    // persist 直前にも dst の非通常ファイル検査を行う。persist(rename) 自体は
    // symlink を辿らず安全に置換できるが、改ざんが起きていたことを運用者に
    // 知らせる。
    remove_dst_if_not_regular_file(dst)?;

    tmp.persist(dst).map_err(|e| {
        anyhow::anyhow!(
            "Failed to atomically replace {} with staged temp file {}: {}",
            dst.display(),
            e.file.path().display(),
            e.error
        )
    })?;

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
/// ステージ済みファイルの mtime がホスト側**以上**の場合、「コンテナが更新した」
/// とみなして保持する(= コピーしない)。ホスト側の mtime が厳密に新しい場合
/// (= ユーザーが再ログインした等)のみ、従来どおりホスト側優先で上書きする。
/// ホストへの書き戻しは行わない(「ホスト原本に触れない」原則のため)。
///
/// 同値をステージ優先に倒す理由(codex レビュー round 8 P2-1): 判定を厳密な
/// `>` にしていると、ステージへのコピーとコンテナ内でのトークンリフレッシュが
/// ファイルシステムのタイムスタンプ分解能内(粗い fs では同一秒)に起きた場合、
/// mtime が同値になり、更新済みのステージが古いホスト認証で上書きされてしまう。
/// 同値でかつ実際にはホストが真に新しい(同一瞬間の再ログイン)ケースは
/// 天文学的に稀であり、仮に起きても再ログインし直せば回復できる。一方、
/// リフレッシュトークン喪失(ステージ優先にしなかった場合に起き得る)は回復に
/// ユーザー操作が必須の重い障害になる。前者の取りこぼしより後者を避ける方が
/// 安全側であるため、同値はステージ優先とする。
pub fn should_keep_staged_auth(
    host_mtime: std::time::SystemTime,
    staged_mtime: std::time::SystemTime,
) -> bool {
    staged_mtime >= host_mtime
}

/// 共有 codex ステージ(`<config_dir>/codex/`)への並行アクセスを直列化する
/// アドバイザリロック(codex レビュー round 7 P1)。
///
/// 複数の `vibepod run` が同時に `prepare_codex_mount` を呼ぶと、一方の完全
/// リコンサイル(`reconcile_codex_stage_dir`)が、他方が書き込み中の
/// `NamedTempFile`(round 6 で一意名にした allowlist 外の一時ファイル)を
/// 「allowlist に無いエントリ」として削除してしまうことがある。一意名
/// (round 6)は symlink 差し替えと固定名の取り合いを防いだが、2 プロセスの
/// 操作順序そのものは制御しないため、この削除競合(reconcile が他方の
/// in-flight tmp ファイルを消す)までは防げない。`prepare_codex_mount` の
/// 本体(リコンサイル開始〜全コピー完了)全体をこのロックで排他区間にする
/// ことで、2 つの呼び出しが同じステージをインターリーブして操作しない
/// ことを保証する。
///
/// UX は `BuildLock`(`src/cli/init.rs`)と同じパターンに倣う:
/// `flock(LOCK_EX | LOCK_NB)` を先に試し、競合(`EWOULDBLOCK`)のときだけ
/// 待機を告知してから `flock(LOCK_EX)` でブロッキング取得する。`BuildLock`
/// はビルド専用の UX 文言・ファイル名(`build.lock`)を持つ private struct
/// であり、無理に汎用化すると既存の init 経路の意味論に codex 側の事情
/// (ファイル名・文言)を持ち込むリスクの方が大きいため、独立実装とする
/// (spec が許容する選択)。
///
/// ロックファイルはステージディレクトリ(`<config_dir>/codex/`)の**外**、
/// `<config_dir>/codex.lock` に置く。ステージ内に置くと
/// `reconcile_codex_stage_dir` の完全リコンサイル(allowlist 外を全削除)が
/// 次回呼び出し時にロックファイル自体を削除対象にしてしまう。
///
/// ロックはファイル記述子を閉じる(戻り値の drop)と解放される。
struct CodexStageLock {
    #[cfg(unix)]
    _file: std::fs::File,
}

/// `config_dir` に対する `CodexStageLock` を取得する。詳細は
/// `CodexStageLock` のドキュメントを参照。
fn acquire_codex_stage_lock(config_dir: &std::path::Path) -> anyhow::Result<CodexStageLock> {
    std::fs::create_dir_all(config_dir).with_context(|| {
        format!(
            "Failed to create config dir for codex stage lock: {}",
            config_dir.display()
        )
    })?;
    let path = config_dir.join("codex.lock");
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("Failed to open codex stage lock file: {}", path.display()))?;

        // まず非ブロッキングで試す。即取れれば(競合なしの通常ケース)何も
        // 出さない。取れない場合だけ「別プロセスが codex ステージを準備中」
        // と stderr に告知してからブロッキングで取り直す(BuildLock と同じ
        // UX パターン)。
        //
        // SAFETY: flock は単純なシステムコール。fd は `file` の生存期間に
        // わたって有効。LOCK_NB は即時に返り、LOCK_EX はブロッキングで排他
        // ロックを取る。
        let fd = file.as_raw_fd();
        let rc_nb = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc_nb != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                eprintln!(
                    "Another process is preparing the codex stage ({}). Waiting for it to finish...",
                    path.display()
                );
                let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
                if rc != 0 {
                    return Err(std::io::Error::last_os_error()).with_context(|| {
                        format!(
                            "Failed to acquire exclusive codex stage lock on {}",
                            path.display()
                        )
                    });
                }
            } else {
                // EWOULDBLOCK 以外の失敗(権限・fd 異常等)はそのまま伝播。
                return Err(err).with_context(|| {
                    format!(
                        "Failed to acquire exclusive codex stage lock on {}",
                        path.display()
                    )
                });
            }
        }
        Ok(CodexStageLock { _file: file })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(CodexStageLock {})
    }
}

/// ホストの `~/.codex/auth.json`(と存在すれば `config.toml`)を
/// `<config_dir>/codex/` にコピーし、そのディレクトリのパスを返す。呼び出し元は
/// このパスをコンテナへ `/home/vibepod/.codex` として **rw** マウントする(codex が
/// トークンリフレッシュ時に auth.json を書き換えるため。コピーなのでホスト原本には
/// 影響しない — `.claude.json` と同じパターン)。
///
/// **全コンテナ共有のユーザー単位ステージ**(`<config_dir>/codex/`)であり、
/// per-container ではない(round 4 で per-container 配置から移行)。per-container
/// 配置だと、disposable 実行(`--new` / worktree)の終了処理が
/// `<config_dir>/runtime/<container_name>/` を丸ごと `remove_dir_all` する際に、
/// コンテナ内 codex がリフレッシュした auth.json(トークンローテーション後の
/// 唯一の有効コピー)ごと失われてしまう。ユーザー単位の共有パスに置くことで、
/// per-container cleanup の削除対象から構造的に外れる。
///
/// **トレードオフ(意図的な受け入れ)**: 複数コンテナを併走させている場合、
/// それらは同一の `auth.json` ステージを共有する。1 つのコンテナ内 codex が
/// トークンをリフレッシュすると、他の実行中コンテナにもそのファイルが反映される。
/// これは codex 側の書き込みが(追記ではなく)ファイル置換であるため実害は限定的で
/// あり、また per-container コピー方式を採ったとしても provider 側のリフレッシュ
/// トークンローテーション自体は同様に起こり得る問題であるため、共有ステージに
/// 集約する方が総合的に安全と判断した。
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
///
/// 関数本体全体(リコンサイル開始〜全コピー完了)は `CodexStageLock`
/// (`<config_dir>/codex.lock`)による排他区間内で実行される(codex レビュー
/// round 7 P1)。複数の `vibepod run` が同時にこの関数を呼んでも、一方の
/// リコンサイルが他方の in-flight temp file を削除する競合が起きない。
pub fn prepare_codex_mount(
    home: &std::path::Path,
    config_dir: &std::path::Path,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    // round 7 P1: リコンサイル開始から全コピー完了まで(この関数の残り
    // 全体、early return を含むすべての return パス)を排他区間にする。
    // ロックガードは関数を抜けるとき(どの return パスでも)に drop され、
    // 解放される。
    let _stage_lock = acquire_codex_stage_lock(config_dir)?;

    let host_codex_dir = home.join(".codex");
    let entries = host_codex_stage_entries(&host_codex_dir);

    let codex_stage_dir = config_dir.join("codex");

    let has_auth = entries.iter().any(|(_, name)| *name == "auth.json");
    if !has_auth {
        // P1: ホストの auth.json が無い(未認証 or 取り消し済み)。過去の run で
        // ステージ済みの認証情報が残っていると、既存コンテナの bind mount
        // 経由で使われ続けてしまうため、ディレクトリ自体は残したまま中身だけ
        // 全消去する(keep が空 = allowlist 全ファイルが削除対象)。
        reconcile_codex_stage_dir(&codex_stage_dir, &[]).with_context(|| {
            format!(
                "Failed to clear stale codex assets in {}",
                codex_stage_dir.display()
            )
        })?;
        eprintln!(
            "codex auth not found (~/.codex/auth.json); codex review is unavailable in this container"
        );
        return Ok(None);
    }

    std::fs::create_dir_all(&codex_stage_dir)
        .with_context(|| format!("Failed to create {}", codex_stage_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&codex_stage_dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| {
                format!("Failed to set permissions on {}", codex_stage_dir.display())
            })?;
    }

    // P2: 今回の entries に無い allowlist ファイル(例: ホストで config.toml が
    // 削除された)がステージに残っていると無期限に使われ続けるため、コピー前に
    // 差分を削除しておく。
    let keep_names: Vec<&str> = entries.iter().map(|(_, name)| *name).collect();
    reconcile_codex_stage_dir(&codex_stage_dir, &keep_names).with_context(|| {
        format!(
            "Failed to reconcile stale codex assets in {}",
            codex_stage_dir.display()
        )
    })?;

    for (src, name) in &entries {
        let dst = codex_stage_dir.join(name);

        // P1-a/P1(round 8): dst はコンテナの rw マウント経由で symlink や
        // ディレクトリ等の非通常ファイルに差し替えられている可能性がある。
        // 以降の mtime 判定・コピーが symlink 経由でホスト任意ファイルに
        // 触れたり、ディレクトリを「新しい auth.json」として誤採用したり
        // しないよう、まず非通常ファイルなら辿らず削除する。以後 dst は
        // 「存在しないか、通常ファイル」として扱ってよい。
        remove_dst_if_not_regular_file(&dst)?;

        if *name == "auth.json" {
            let staged_meta = match std::fs::symlink_metadata(&dst) {
                Ok(meta) => Some(meta),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    return Err(e).with_context(|| format!("Failed to stat {}", dst.display()))
                }
            };

            if let Some(meta) = staged_meta {
                // dst が symlink でないと分かった上での symlink_metadata なので、
                // 通常ファイルの mtime としてそのまま使える(symlink_metadata と
                // metadata は非 symlink に対して同じ mtime を返す)。
                let staged_mtime = meta
                    .modified()
                    .with_context(|| format!("Failed to read mtime of {}", dst.display()))?;
                let host_mtime = std::fs::metadata(src)
                    .and_then(|m| m.modified())
                    .with_context(|| format!("Failed to read mtime of {}", src.display()))?;

                if should_keep_staged_auth(host_mtime, staged_mtime) {
                    // コンテナ内 codex がトークンリフレッシュ済みの auth.json を、
                    // 古いホストコピーで上書きしない(round 3 P1)。コンテナが
                    // この経路でパーミッションだけ緩めていた場合の修復は、この
                    // ループを抜けた後の `enforce_staged_permissions` に集約する
                    // (round 9 P1)。
                    continue;
                }
            }
        }

        copy_codex_asset_atomically(src, &dst)?;
    }

    enforce_staged_permissions(&codex_stage_dir, &entries)?;

    Ok(Some(codex_stage_dir))
}

/// ステージ内の allowlist 対象ファイル(`entries` に列挙された名前、通常
/// `auth.json` と、存在すれば `config.toml`)がすべて 0600 であることを
/// 保証する(codex レビュー round 9 P1)。
///
/// `prepare_codex_mount` のループには、ステージ済みファイルの中身・
/// パーミッションを意図的に変更せず次へ進む経路が2つある:
///
/// - keep-newer 経路: `should_keep_staged_auth` が true を返し `continue`
///   する(コンテナがリフレッシュした auth.json をホストコピーで上書きしない)
/// - コピー省略経路: `copy_codex_asset_atomically` 内の
///   `staged_asset_matches_host` が true を返し、コピー・chmod・rename を
///   一切行わず早期 return する(内容が既にホストと同一)
///
/// コンテナは rw マウント経由でステージ済みファイルのパーミッションだけを
/// 緩める(例: `chmod 0644 auth.json`)ことができる。この chmod は上記2経路
/// のどちらが判定されるかに影響しない(mtime も内容も変えないため)ので、
/// 一度緩められた権限は当該ファイルが実際にコピーし直されるまで検出も修復も
/// されず、同一ホスト上の別ユーザーがトークンを読める状態が恒久的に残って
/// しまう。
///
/// これを塞ぐため、コピー実施済み・keep 済み・skip 済みのいずれの経路を
/// 通ったファイルにも、`prepare_codex_mount` の成功 return 直前で一律に
/// 0600 を再設定する。chmod は冪等なので、既に 0600 のファイルに対して
/// 実行しても無害であり、mtime 変更等の副作用も無い。
///
/// `symlink_metadata` + `set_permissions(path, ..)` の check-then-act では
/// TOCTOU レースが残る(codex レビュー round 9 P1 差し戻し、指摘必須対応):
/// `codex_stage_dir` はコンテナが rw マウント経由で同時に書き換えられる
/// 共有ディレクトリであり、まさにこの関数が守ろうとしている脅威モデル
/// そのものである。`std::fs::set_permissions` は Unix では `chmod(path, ..)`
/// を呼ぶため symlink を辿ってしまう(`lchmod` 相当は std に無い)。したがって
/// `symlink_metadata` で「通常ファイルだ」と確認した直後、`set_permissions`
/// が実際に path を解決するまでの一瞬にコンテナが `dst` を任意ホストパスへの
/// symlink に差し替えると、chmod がそのリンクを辿ってホスト側の任意ファイルの
/// パーミッションを書き換える(confused deputy)。
///
/// これを避けるため、`O_NOFOLLOW` で `dst` を開いて得た fd に対して
/// `File::set_permissions`(内部で `fchmod(fd, ..)` を呼ぶ、path 解決を伴わない)
/// で権限を変更する。open 自体が「symlink なら ELOOP で失敗する」という形で
/// symlink 検出込みでアトミックに行われるため、`copy_codex_asset_atomically`
/// の `persist`(rename ベースで symlink を辿らない)や
/// `remove_dst_if_not_regular_file`(`remove_file`/`remove_dir_all` で symlink
/// を辿らない)と同じく、この関数も symlink 追従を構造的に排除する。
///
/// FIFO 等の特殊ファイルを `O_NOFOLLOW` だけで開くと、読み手が付くまで open
/// 自体がブロックしてしまう可能性があるため `O_NONBLOCK` も併用する(通常
/// ファイルに対しては no-op)。
///
/// ループ内で各エントリは既に `remove_dst_if_not_regular_file` を通過済み
/// (=存在しないか通常ファイル)のはずだが、この関数自体は「open した時点で
/// 通常ファイルか」を独立に確認する(二重防御。かつ `remove_dst_if_not_regular_file`
/// 通過からこの関数の呼び出しまでの間にも同じ TOCTOU の窓があり得るため、
/// ここでの再確認そのものが本質的な防御になる)。
fn enforce_staged_permissions(
    codex_stage_dir: &std::path::Path,
    entries: &[(std::path::PathBuf, &'static str)],
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        for (_, name) in entries {
            let dst = codex_stage_dir.join(name);

            let file = match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&dst)
            {
                Ok(file) => file,
                // 今回の run では扱われなかった(entries には出るが host 側に
                // 実体が無かった)か、この関数に来るまでの間に何らかの理由で
                // 消えた。対象外として静かにスキップしてよい。
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                // symlink だった場合の open(2) の挙動(ELOOP)。
                // `remove_dst_if_not_regular_file` 通過後からこの open までの
                // 間にコンテナが symlink へ差し替えた可能性がある。辿らず
                // 検出できた時点でこの防御の目的は達成しているため chmod は
                // 行わず、運用者が調査できるよう stderr に記録するに留める。
                Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                    eprintln!(
                        "staged codex file was replaced with a symlink just before permission \
                         enforcement (possible concurrent container tampering); skipped chmod \
                         to avoid following it: {}",
                        dst.display()
                    );
                    continue;
                }
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to open {} for chmod", dst.display()))
                }
            };

            let file_type = file
                .metadata()
                .with_context(|| {
                    format!(
                        "Failed to stat opened fd for {} before chmod",
                        dst.display()
                    )
                })?
                .file_type();
            // O_NOFOLLOW は symlink を弾くが、ディレクトリ等の他の非通常
            // ファイルはそのまま開けてしまう。通常ファイルでなければ
            // `remove_dst_if_not_regular_file` と同じ方針で chmod せず対象外
            // とする。
            if !file_type.is_file() {
                continue;
            }

            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| {
                    format!("Failed to enforce 0600 permissions on {}", dst.display())
                })?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (codex_stage_dir, entries);
    }
    Ok(())
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

    #[test]
    fn copy_codex_asset_atomically_survives_concurrent_calls_to_same_destination() {
        // codex レビュー round 6 P2: 固定名 `<name>.tmp` を使う旧実装では、
        // 複数の `vibepod run` が同時にステージ準備を行うと同じ一時ファイル名を
        // 取り合い、一方が rename した直後に他方の chmod/rename が NotFound で
        // 不定期に失敗していた。NamedTempFile::new_in は呼び出しごとに OS レベルで
        // 排他的に一意な名前を生成するため、同じ dst へ並行に呼び出しても
        // 双方が(競合エラーなく)成功することを確認する。
        let stage_dir = tempfile::tempdir().expect("failed to create stage tempdir");
        let src_dir = tempfile::tempdir().expect("failed to create src tempdir");

        let src_a = src_dir.path().join("a.json");
        let src_b = src_dir.path().join("b.json");
        std::fs::write(&src_a, "FROM_A").expect("failed to write src_a");
        std::fs::write(&src_b, "FROM_B").expect("failed to write src_b");

        let dst = stage_dir.path().join("auth.json");
        let dst_a = dst.clone();
        let dst_b = dst.clone();

        let handle_a = std::thread::spawn(move || copy_codex_asset_atomically(&src_a, &dst_a));
        let handle_b = std::thread::spawn(move || copy_codex_asset_atomically(&src_b, &dst_b));

        let result_a = handle_a.join().expect("thread A must not panic");
        let result_b = handle_b.join().expect("thread B must not panic");

        assert!(
            result_a.is_ok(),
            "concurrent copy A must not fail on a fixed-name tmp file collision: {:?}",
            result_a.err()
        );
        assert!(
            result_b.is_ok(),
            "concurrent copy B must not fail on a fixed-name tmp file collision: {:?}",
            result_b.err()
        );

        // Which writer "won" the final rename is an intentional race (both are
        // valid outcomes); what matters is that dst always ends up as exactly
        // one complete writer's content, never a partial write or a leftover
        // tmp file at a colliding fixed name.
        let final_content = std::fs::read_to_string(&dst)
            .expect("dst must exist and be readable after both copies");
        assert!(
            final_content == "FROM_A" || final_content == "FROM_B",
            "dst must contain exactly one of the two concurrent writers' content, got: {final_content:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_codex_asset_atomically_ignores_hostile_fixed_name_tmp_symlink() {
        // codex レビュー round 6 P1: 固定名 `<name>.tmp`(例: `auth.json.tmp`)を
        // 使う旧実装は、`std::fs::copy(src, &tmp)` の時点で `tmp` を辿ってしまう。
        // コンテナが rw マウント経由でその固定名を先回りしてホスト任意パスへの
        // symlink にしておくと、次回の copy がリンク先(ホスト側ファイル)を
        // 直接上書きしてしまい、dst 自体の非通常ファイル防御
        // (remove_dst_if_not_regular_file)を完全に迂回できた。
        //
        // `prepare_codex_mount` 経由の統合テストでは、この関数呼び出しより前に
        // round 5 の完全リコンサイル(`reconcile_codex_stage_dir`)が allowlist 外
        // エントリ(`auth.json.tmp` を含む)を掃除してしまうため、copy 機構自体が
        // 固定名を使わなくなったこと(round 6 の本体修正)を判別できない。実際、
        // reconcile を通すテストは copy 側を旧実装に戻しても reconcile の効果で
        // 偽陽性に pass してしまう。そのためこのテストは reconcile を経由せず
        // `copy_codex_asset_atomically` を直接呼び出し、copy 機構そのものが
        // 固定名 tmp ファイルを一切使用しないことを検証する。
        let stage_dir = tempfile::tempdir().expect("failed to create stage tempdir");
        let victim_dir = tempfile::tempdir().expect("failed to create victim tempdir");
        let src_dir = tempfile::tempdir().expect("failed to create src tempdir");

        let src = src_dir.path().join("auth.json");
        std::fs::write(&src, r#"{"token":"HOST_AUTH"}"#).expect("failed to write src");

        let dst = stage_dir.path().join("auth.json");

        // A "victim" file outside the stage, standing in for an arbitrary
        // host path a container might target via the fixed-name tmp file
        // (e.g. ~/.ssh/authorized_keys).
        let victim = victim_dir.path().join("victim.txt");
        std::fs::write(&victim, "VICTIM_UNCHANGED").expect("failed to write victim");

        // Plant the hostile fixed-name tmp file that the pre-round-6
        // implementation would have written through, pointing it at the
        // victim, directly in the same directory `copy_codex_asset_atomically`
        // writes to. No reconcile step runs in this test, so this is exactly
        // what a container could leave behind between two `vibepod run`
        // invocations.
        let hostile_tmp = stage_dir.path().join("auth.json.tmp");
        std::os::unix::fs::symlink(&victim, &hostile_tmp).expect("failed to plant hostile symlink");
        assert!(
            std::fs::symlink_metadata(&hostile_tmp)
                .expect("hostile tmp must exist")
                .file_type()
                .is_symlink(),
            "sanity check: the hostile auth.json.tmp must actually be a symlink before the copy"
        );

        copy_codex_asset_atomically(&src, &dst)
            .expect("copy must succeed despite the hostile fixed-name tmp file");

        assert_eq!(
            std::fs::read_to_string(&victim).expect("victim must still be readable"),
            "VICTIM_UNCHANGED",
            "the hostile fixed-name tmp file's symlink target must never be written to by the \
             copy machinery itself"
        );
        assert_eq!(
            std::fs::read_to_string(&dst).expect("dst must be staged"),
            r#"{"token":"HOST_AUTH"}"#,
            "dst must be correctly staged from src despite the hostile fixed-name tmp file \
             sharing its directory"
        );
    }
}
