use anyhow::{Context, Result};

use crate::runtime::{ContainerConfig, ContainerStatus};
use crate::session::SessionStore;

mod interactive;
pub mod lock;
pub mod prepare;
mod prompt;

/// run 後の後片付け時点で、コンテナが停止/削除確定か、稼働継続の可能性が
/// あるかを表す(codex レビュー round 11 P1 で導入、round 12 P1-b で意味を
/// 「後片付けでステージを削除してよいか」の判断に一本化)。
///
/// ステージの `auth.json` はコンテナが rw マウント経由で書き込む untrusted な
/// 入力である。auth store への同期は liveness に関わらず常に「完全な JSON か」を
/// 検証してから行う(round 12 P1-a: `docker stop` が SIGKILL に落ちると停止済み
/// でもステージが truncated で残り得るため、検証を liveness に依存させない)。
/// この enum が決めるのは検証の有無ではなく、**後片付けでステージを含む runtime
/// dir を削除してよいか**である。
///
/// - `Stopped`: `docker stop` / `docker rm -f` の exit status で停止/削除が確定
///   した経路。コンテナはもうステージに書き込まないため、同期後に runtime dir を
///   丸ごと削除してよい。
/// - `Running`: コンテナが稼働継続する経路(interactive の再利用コンテナ)、または
///   `docker rm -f` / `stop` が失敗してコンテナが生存し得る経路。ステージへの並行
///   書き込みが起こり得るため、同期(検証付き)は行うが runtime dir は削除せず保持
///   し、次回 run に後片付けを委ねる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerLiveness {
    /// コンテナは停止/削除確定。同期後に runtime dir を削除してよい。
    Stopped,
    /// コンテナが稼働継続の可能性あり。同期はするが runtime dir は保持する。
    Running,
}

/// CLI `run` サブコマンドのオプション
///
/// `vibepod run` コマンドの全引数を保持する。
pub struct RunOptions {
    pub resume: bool,
    pub prompt: Option<String>,
    /// `--prompt-file <path>`: プロンプトをファイルから読み込む。`--prompt` と
    /// clap レベルで排他（`conflicts_with = "prompt"`）。`execute()` の冒頭で
    /// `resolve_prompt_file` により読み込まれ、無加工のまま `prompt` へ格納
    /// される。以降の全経路は `--prompt` と完全に同一のため、この field を
    /// 直接参照するコードは `execute()` 以外に存在しない。
    pub prompt_file: Option<String>,
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

/// タイムアウト種別（idle / overall）の日本語ラベルを一箇所で定義する。
/// `render_timeout_message` と、タイムアウト時に `prompt.rs` が最終的に返す
/// `anyhow::bail!` のエラー理由の両方がこれを参照することで、表示文言が
/// 食い違う（片方だけ更新し忘れる）事故を防ぐ。
pub fn timeout_kind_label(overall_timed_out: bool) -> &'static str {
    if overall_timed_out {
        "実時間上限"
    } else {
        "ストリーム無出力"
    }
}

/// タイムアウト上限値を「N 分」「N 秒」「N 分 M 秒」の日本語表記にする純関数。
///
/// F9（フル再レビュー指摘）: 旧実装は `limit_secs / 60` の整数除算のみで
/// 分表記を作っており、`--timeout 90` のような端数を持つ値が「1 分」に
/// 切り捨てられ、実際に待たされた 30 秒分の情報が消えていた。60 の倍数
/// ちょうどのときだけ「N 分」、端数があるときは「N 分 M 秒」、60 秒未満は
/// 「N 秒」にする。
fn format_timeout_duration(limit_secs: u64) -> String {
    if limit_secs < 60 {
        return format!("{} 秒", limit_secs);
    }
    let minutes = limit_secs / 60;
    let remainder_secs = limit_secs % 60;
    if remainder_secs == 0 {
        format!("{} 分", minutes)
    } else {
        format!("{} 分 {} 秒", minutes, remainder_secs)
    }
}

/// タイムアウト時点の workspace 状態。`render_timeout_message` の案内内容を
/// 決める唯一の入力（`worktree_dir` を除く）。
///
/// Copilot 指摘 + reviewer mn2（フル再レビュー後、独立に再検出）: 以前は
/// `head_advanced: bool` / `has_uncommitted: bool` の2引数で表現しており、
/// 呼び出し元（`prompt.rs`）は probe（`git::get_head_hash` /
/// `git::try_has_uncommitted_changes`）が失敗した場合に
/// `has_uncommitted.unwrap_or(true)` のようなフォールバック値を埋めていた。
/// 「true 決め打ち」は一見安全側に見えるが、実際には存在しない
/// `vibepod restore` 不可の理由を語ったり、そもそも「未コミットあり」と
/// 断定すること自体が probe が実際に確認できていない事実を語ることになる
/// （握り潰しの一種）。`Unknown` を独立した状態として持つことで、
/// 「確認できなかった」という事実そのものを案内でき、bool 2つの組み合わせ
/// では表現できない第4の状態を型で表現する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutWorkspaceState {
    /// 未コミットの変更が残っている（probe 成功）。
    Uncommitted,
    /// コミット済みの変更のみで、ツリーは clean（probe 成功）。
    CommittedClean,
    /// 開始時点から変更なし（probe 成功）。
    Unchanged,
    /// probe が失敗し、状態を確認できなかった（spawn 失敗・git 非ゼロ終了）。
    Unknown,
}

/// タイムアウト時に stderr へ出す打ち切りメッセージを組み立てる純関数。
///
/// 要件2（設計書 第3節: タイムアウト時の workspace 保全）: タイムアウト時は
/// workspace の git 状態を一切変更しないため、この関数は git コマンドを一度も
/// 呼ばない。呼び出し元（`prompt.rs`）が read-only な `git::get_head_hash` /
/// `git::try_has_uncommitted_changes` で先に状態を調べ、その結果を
/// `TimeoutWorkspaceState` としてここへ渡す（この関数自体は純関数のまま
/// 維持する）。この doc コメントが案内文言・各コマンドを選んだ判断根拠の
/// 正本（設計spec 3.2節はここへの参照のみを持つ）。
///
/// 案内は `TimeoutWorkspaceState` の4状態（F2 で導入した3状態 + Copilot/mn2
/// で追加した `Unknown`）を軸に**1回だけ**書く。`worktree_dir` が
/// `Some(dir)` のとき（F3）は、確認・破棄コマンドの対象を `.worktrees/<dir>`
/// （cwd とは別の git worktree）へ切り替えるだけで、状態分岐の構造自体は
/// cwd と共有する — `git` クロージャが `worktree_dir` の有無で
/// `git -C .worktrees/<dir> <args>` / `git <args>` を組み立てる。
///
/// Q3（フル再レビュー後 simplify 指摘）: 以前は `match worktree_dir { Some
/// => {状態分岐}, None => {状態分岐} }` という二重実装で、cwd/worktree
/// 双方にほぼ同じ分岐を別々に書いていた。この二重化自体が実装漏れの温床に
/// なっており、実際に worktree の「未コミットあり」分岐にだけ `log` が
/// 無い・「コミット済みのみ」分岐にだけ `status` が無いという非対称
/// （本来どちらのモードでも確認手段は同じであるべき）が生じていた。状態
/// 分岐を1本化し、worktree/cwd の違いは `git` クロージャと状態末尾の締めの
/// 文言だけに閉じ込めることで、この種の漏れが構造的に起きなくなる。
///
/// 各状態の内容:
/// - **`Uncommitted`（未コミット変更あり）**: `vibepod restore` は未コミット
///   変更が残っていると必ず bail する（`git status --porcelain` が非空なら
///   "Uncommitted changes detected" で失敗）ため、cwd では使えない旨を明記
///   する。確認コマンド（`status` / `log`、両モード対称）、破棄コマンド
///   （MJ1: 引数無し `reset --hard`（HEAD を動かさず index + working tree
///   を HEAD の状態へ戻す）と `clean -fd` の組み合わせ — 以前の
///   `checkout .` は index から working tree を復元するだけでステージ済み
///   変更を戻せず、`clean -fd` も `git add` 済みファイルを消せなかった
///   ため、案内どおり破棄しても bail が再現していた）を示す。締めは cwd
///   なら保持/コミットの選択肢、worktree なら保持の選択肢に加え worktree
///   削除コマンド（mn1: dirty な worktree への素の `git worktree remove`
///   は "contains modified or untracked files, use --force to delete it"
///   で必ず失敗するため `--force` 付き）。
/// - **`CommittedClean`（コミット済みのみ・clean）**: 確認コマンド
///   （`status` / `log`）に加え、worktree のみ `diff main` も案内する
///   （cwd は自分自身のブランチとの diff になり無意味なため、worktree
///   固有の価値がある `diff main` だけを意図的に非対称のまま残す —
///   実装漏れではない）。締めは cwd なら `vibepod restore`、worktree なら
///   （clean なので `--force` 無しの）`git worktree remove`。
/// - **`Unchanged`（開始時点から無変更）**: その旨のみ。`vibepod restore`
///   も worktree 削除コマンドも、それぞれ案内する価値がない cwd/worktree
///   側では出さない（worktree 削除は無変更でも案内する — 掃除のため）。
/// - **`Unknown`（probe 失敗、Copilot 指摘 + reviewer mn2）**: 状態を一切
///   断定しない。「確認できなかった」旨と、手動確認コマンド（`status`)の
///   みを示す。`vibepod restore`・破棄コマンド・worktree 削除コマンドは
///   一切出さない — dirty かどうか分からない workspace に対してこれらを
///   勧めると、実際には未コミット変更が残っているのに `vibepod restore`
///   を試みて予期しない bail を招いたり、`git worktree remove`（force
///   無し）が失敗したり、最悪 `--force` 付きの破棄コマンドが実在する変更を
///   握り潰したりし得るため。
///
/// 表示層のロジックを I/O から切り離してユニットテスト可能にするため、必要な
/// 値はすべて引数で受け取る（`render_run_summary` と同じパターン）。
#[allow(clippy::too_many_arguments)]
pub fn render_timeout_message(
    overall_timed_out: bool,
    idle_timeout_secs: u64,
    overall_timeout_secs: u64,
    log_path: Option<&std::path::Path>,
    state: TimeoutWorkspaceState,
    worktree_dir: Option<&str>,
) -> String {
    let kind_label = timeout_kind_label(overall_timed_out);
    let limit_secs = if overall_timed_out {
        overall_timeout_secs
    } else {
        idle_timeout_secs
    };
    let timeout_display = format_timeout_duration(limit_secs);

    let mut out = format!(
        "⚠ {}が{} を超えたため、セッションを中断しました。\n",
        kind_label, timeout_display
    );
    if let Some(p) = log_path {
        out.push_str(&format!("  ログ: {}\n", p.display()));
    }

    // cwd は素の `git <args>`、worktree は `.worktrees/<dir>` を明示的に
    // 対象にする `git -C .worktrees/<dir> <args>` を組み立てる（Q3）。
    let git = |args: &str| match worktree_dir {
        Some(dir) => format!("git -C .worktrees/{dir} {args}"),
        None => format!("git {args}"),
    };

    if let Some(dir) = worktree_dir {
        out.push_str(&format!(
            "  作業は `.worktrees/{}`（cwd とは別の git worktree）に残っています。\n",
            dir
        ));
    }

    match state {
        TimeoutWorkspaceState::Uncommitted => {
            out.push_str(if worktree_dir.is_some() {
                "  未コミットの変更があります。\n"
            } else {
                "  エージェントによる変更（コミット・未コミットとも）は workspace にそのまま残っています。\n"
            });
            out.push_str(&format!(
                "  確認するには: `{}` / `{}`\n",
                git("status"),
                git("log")
            ));
            if worktree_dir.is_none() {
                out.push_str(
                    "  `vibepod restore` は未コミットの変更が残っていると実行できません。\n",
                );
            }
            out.push_str(&format!(
                "  破棄するには（取り消し不能です）: `{} && {}`\n",
                git("reset --hard"),
                git("clean -fd")
            ));
            match worktree_dir {
                None => out.push_str(
                    "  残したい場合は、そのまま保持するか `git add -A && git commit` でコミットしてから `vibepod restore` を使ってください。",
                ),
                Some(dir) => {
                    out.push_str(
                        "  残したい場合は、そのまま保持するかそのブランチ内でコミットしてください。\n",
                    );
                    out.push_str(&format!(
                        "  未コミットの変更があるため、削除する場合は先に上記の破棄を行うか \
                         `git worktree remove --force .worktrees/{}` を使ってください。",
                        dir
                    ));
                }
            }
        }
        TimeoutWorkspaceState::CommittedClean => {
            out.push_str(if worktree_dir.is_some() {
                "  変更はコミット済みです。\n"
            } else {
                "  エージェントによる変更はコミット済みで workspace にそのまま残っています。\n"
            });
            out.push_str(&format!(
                "  確認するには: `{}` / `{}`\n",
                git("status"),
                git("log")
            ));
            match worktree_dir {
                None => out.push_str("  開始時点へ戻すには: `vibepod restore`"),
                Some(dir) => {
                    // worktree 固有: main との差分は worktree だけの価値がある
                    // （cwd は自分自身との diff になり無意味）。意図的な非対称。
                    out.push_str(&format!("  main との差分: `{}`\n", git("diff main")));
                    out.push_str(&format!(
                        "  worktree を削除するには: `git worktree remove .worktrees/{}`",
                        dir
                    ));
                }
            }
        }
        TimeoutWorkspaceState::Unchanged => match worktree_dir {
            None => out.push_str("  開始時点から workspace に変更はありません。"),
            Some(dir) => {
                out.push_str("  開始時点から変更はありません。\n");
                out.push_str(&format!(
                    "  worktree を削除するには: `git worktree remove .worktrees/{}`",
                    dir
                ));
            }
        },
        TimeoutWorkspaceState::Unknown => {
            // Copilot 指摘 + reviewer mn2: probe 失敗を「変更なし」等に誤断定
            // せず、確認できなかった旨だけを伝える。`vibepod restore` も
            // 破棄コマンドも worktree 削除コマンドも一切出さない（dirty
            // かどうか分からない workspace に対して安全と誤解させないため）。
            out.push_str(
                "  git の状態を確認できませんでした。変更が残っている可能性があるため、workspace はそのまま保持しています。\n",
            );
            out.push_str(&format!("  手動で確認してください: `{}`", git("status")));
        }
    }

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
    /// **per-container** ステージディレクトリ(`<runtime_dir>/codex/`)。
    /// コンテナへ `/home/vibepod/.codex` として rw マウントされる実体はこれ
    /// であり、コンテナが書き込む実行データ(history/sessions/cache 等)は
    /// このコンテナ専用領域に閉じ、他コンテナから不可視になる(round 10)。
    /// disposable 実行(`--new` / worktree)の `runtime_dir` 削除でこのステージ
    /// ごと消える — それは意図した挙動であり問題ではない: 認証の永続化は
    /// 別途 `<config_dir>/codex-auth/`(host-only、コンテナに一切マウントしない
    /// auth store)が担い、run 後に `sync_codex_stage_to_store` がステージから
    /// store へ書き戻す。存在しない場合(auth.json 欠如)は `None` —
    /// コンテナには codex 認証を注入しない。
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
    /// コンテナ作成・`ensure_image_available` に実際に使うイメージ名。
    /// `profile` が `Some` のときは `image_for_profile(&global_config.image,
    /// profile)`（`global_config` は `prepare_context` 内のローカル変数）で
    /// 導出した profile 付きイメージ名、`None` のときは
    /// `global_config.image` と同じ値。コンテナ作成側（`build_container_config`
    /// の呼び出し側）はこのフィールドを使う — `global_config.image` を直接
    /// 使うと profile が無視される。
    pub(super) effective_image: String,
    /// `.vibepod/config.toml` の `[run] profile`（プロジェクト優先でマージ済み、
    /// `prepare_context` で検証済み）。`None` は profile 未指定。ラベル
    /// （`vibepod.profile`）の生成に使う。
    pub(super) profile: Option<String>,
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
    /// `~/.claude/plugins/data` の per-container 書き込み可能ステージ等、
    /// `:ro` を付けずにマウントする必要があるエントリ。`extra_mounts` とは
    /// 別枠で持つ（`extra_mounts` は `to_create_args()` で一律 `:ro` になる
    /// ため）。`build_container_config` がそのまま `ContainerConfig.rw_mounts`
    /// へ渡す。
    pub(super) rw_mounts: Vec<(String, String)>,
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

/// サニタイズ済み settings.json のコンテナ側マウント先。
/// `prepare_sanitized_settings_mount` の実装と `build_config_labels`（`mounts_label_parts`
/// 経由）のラベル判定の双方が参照する。値がずれるとラベル判定が実際のマウント先を
/// 検出できなくなるため、リテラルの二重管理を避けて一箇所に集約している。
const SANITIZED_SETTINGS_CONTAINER_PATH: &str = "/home/vibepod/.claude/settings.json";

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

/// ラベル中で「`~/.claude/plugins/data` を per-container の書き込み可能
/// ステージへ差し替えている」ことを示すマーカー。bind mount はコンテナ
/// 作成時に固定されるため、これをラベルに含めないと、この修正前に作られた
/// 既存コンテナが「構成に差分なし」と判定され、rw マウントが無いまま
/// 再利用されてしまう（既存コンテナは plugins/data が親の ro マウント内に
/// 閉じたままで、codex plugin 等がコンテナ内で書き込みに失敗し続ける）。
/// 形式が `host:container` の通常マウント表現と衝突しないように専用 prefix
/// を付けている（`SANITIZED_SETTINGS_LABEL_MARKER` と同じパターン）。
pub(super) const PLUGINS_DATA_MOUNT_LABEL_MARKER: &str =
    "plugins_data_rw=/home/vibepod/.claude/plugins/data";

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

/// コンテナ内 Claude Code が `$HOME/.claude/plugins/data` として読むデフォルトパス。
const DEFAULT_PLUGINS_DATA_CONTAINER_PATH: &str = "/home/vibepod/.claude/plugins/data";

/// `~/.claude/plugins/data` に対応する rw マウントエントリを返す（ホスト側
/// ステージの用意は呼び出し側 `prepare_plugins_data_mount` の責務）。
///
/// 親の `~/.claude/plugins` は read-only でマウントされる
/// (`plugins_mount_entries`) が、codex plugin 等はその内側の `data/` 配下に
/// ジョブ状態ディレクトリを作ろうとして必ず失敗する。この関数は `data/`
/// サブディレクトリだけを per-container の書き込み可能ステージへ差し替える
/// ためのマウントエントリを返す。`plugins_mount_entries` と完全に対称な実装
/// にしている（二重マウントが必要な理由も同じ）:
///
/// 1. `/home/vibepod/.claude/plugins/data` — Claude Code が $HOME 経由で読む先
/// 2. `<host_home>/.claude/plugins/data` — `installed_plugins.json` の
///    `installPath` フィールドがホスト絶対パスを持つため、同じ絶対パスに
///    再マウントして解決する
///
/// ホスト HOME が `/home/vibepod` の場合、(1) と (2) のコンテナ側パスが一致する
/// ため (2) を追加せず 1 本だけ返す（docker run -v が同一マウント先を拒否する）。
pub fn plugins_data_mount_entries(
    stage_host: &str,
    home: &std::path::Path,
) -> Vec<(String, String)> {
    let mut entries = Vec::with_capacity(2);
    entries.push((
        stage_host.to_string(),
        DEFAULT_PLUGINS_DATA_CONTAINER_PATH.to_string(),
    ));
    let absolute_container = home.join(".claude").join("plugins").join("data");
    if absolute_container != std::path::Path::new(DEFAULT_PLUGINS_DATA_CONTAINER_PATH) {
        entries.push((
            stage_host.to_string(),
            absolute_container.to_string_lossy().to_string(),
        ));
    }
    entries
}

/// `~/.claude/plugins/data` を per-container の空の書き込み可能ステージへ
/// 差し替えるための準備を行う。
///
/// **背景**: `~/.claude/plugins` は read-only でコンテナへマウントされる
/// (`build_claude_config_mounts` / `plugins_mount_entries`) が、codex plugin
/// 等は `plugins/data/<...>/state/<workspace>/jobs` のようなジョブ状態
/// ディレクトリを作ろうとして常に失敗する（親が ro のため）。かといって
/// ホストの `plugins/data` 配下をそのまま rw でマウントすると、他プロジェクト
/// の codex job 履歴等がそのままコンテナへ持ち込まれてしまう
/// （`HOST_CLAUDE_ALLOWLIST` が `sessions/` `projects/` を意図的に除外して
/// いるのと同じ理由で避けたい）。そこで、ホスト内容は一切コピーせず、
/// per-container の**空の**ステージを用意してそこへ書き込ませる。
///
/// **挙動**:
/// 1. `home/.claude/plugins` がディレクトリでなければ `Ok(None)`
///    （plugins 自体をマウントしないケースであり、data の rw ステージも不要）。
/// 2. ホスト側 mountpoint `home/.claude/plugins/data` を（無ければ）作成する。
///    これは Claude Code 自身が作る標準ディレクトリであり、空で作成しても
///    副作用がない。子ディレクトリへの bind mount は docker がホスト側に
///    実体ディレクトリを要求するため必須の準備。**失敗してもビルド全体は
///    落とさない** — codex plugin 等が書き込みに失敗し得る旨を stderr に
///    警告したうえで `Ok(None)` を返す（rw マウントを諦めるだけで run 自体は
///    継続できるため。CLAUDE.md: エラーを握りつぶさず運用者が次の判断を
///    できる情報を出す）。
/// 3. per-container ステージ `runtime_dir/plugins-data` を作成する
///    （**空のまま。ホストの内容はコピーしない**）。こちらは vibepod が
///    書き込み権限を持つはずの領域（`runtime_dir` 配下）の準備であり、
///    失敗を握りつぶすと以後の run 準備が不整合な状態のまま進んでしまうため、
///    `?` でそのまま呼び出し元へ伝播する。
/// 4. ステージの絶対パス文字列を `Ok(Some(..))` で返す。
pub fn prepare_plugins_data_mount(
    home: &std::path::Path,
    runtime_dir: &std::path::Path,
) -> anyhow::Result<Option<String>> {
    let plugins_dir = home.join(".claude").join("plugins");
    if !plugins_dir.is_dir() {
        return Ok(None);
    }

    let host_data_dir = plugins_dir.join("data");
    if let Err(e) = std::fs::create_dir_all(&host_data_dir) {
        eprintln!(
            "warning: failed to create {} ({e}); ~/.claude/plugins/data will remain read-only \
             in the container, so plugins that write there (e.g. codex) may fail to write",
            host_data_dir.display()
        );
        return Ok(None);
    }

    let stage_dir = runtime_dir.join("plugins-data");
    std::fs::create_dir_all(&stage_dir).with_context(|| {
        format!(
            "Failed to create plugins/data stage directory {}",
            stage_dir.display()
        )
    })?;

    Ok(Some(stage_dir.to_string_lossy().to_string()))
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
        SANITIZED_SETTINGS_CONTAINER_PATH.to_string(),
    )))
}

/// ホストの `~/.codex/` 配下でコンテナに持ち込んでよい資産の **allowlist**。
/// `auth.json` と `config.toml` の 2 つのみ。
///
/// **意図的に除外しているもの**: `history.jsonl` / `goals_*.sqlite` / `cache/` 等。
/// 機微・不要データであり、codex レビューの実行に一切必要ないため。
pub const HOST_CODEX_ALLOWLIST: &[&str] = &["auth.json", "config.toml"];

/// `codex_dir`(= `<home>/.codex`)配下で、allowlist に載っていてかつ実際に
/// 存在する**通常ファイル**を `(絶対パス, ファイル名)` で返す。存在しないもの
/// (`NotFound`)は黙ってスキップする(`config.toml` が無いホストは異常では
/// なく通常の運用)。
///
/// symlink は `symlink_metadata`(リンクを辿らない)で判定して**除外**する
/// (codex レビュー round 13 P1)。`is_file()` はリンクを辿ってしまうため、
/// ホストの `auth.json` / `config.toml` が別ファイルへの symlink だと
/// リンク先の内容が検証なしで store/コンテナへ流れてしまい、SECURITY.md の
/// 「コピー元も通常ファイルのみ・symlink 拒否」と矛盾する。symlink を検出
/// した場合は無言で捨てず、dotfiles 管理ツール等で symlink 運用している
/// ユーザーが原因に気づけるよう stderr に警告する。
///
/// この関数は host 側 `~/.codex` と store_dir の両方の列挙に使われる。
/// 両方に同じ symlink 拒否が効くのは意図通り(store 内が symlink に
/// 差し替えられていてもリンク先を流さないため)。
///
/// **NotFound 以外の stat エラーは wipe を誘発させるため fail-fast する
/// (Copilot レビュー指摘、PR #56)**: 以前はここで `eprintln!` するだけで
/// NotFound と同じ「エントリなし」として扱っていたため、呼び出し元
/// `prepare_codex_mount` の `has_auth` 判定が「権限エラーで stat 失敗」を
/// 「auth.json が存在しない」と誤認し、P1 分岐(auth store・ステージの
/// 全消去)を誤発動させ得た。実際には存在するが読めなかっただけの
/// `auth.json` に対して、過去 run で保存済みの有効なトークンを消してしまう
/// のは許容できない。NotFound(本当に未認証、通常運用)だけを黙ってスキップし、
/// それ以外は run 準備自体を失敗させて運用者に原因(権限等)を伝える。
pub fn host_codex_stage_entries(
    codex_dir: &std::path::Path,
) -> anyhow::Result<Vec<(std::path::PathBuf, &'static str)>> {
    // `std::fs::symlink_metadata` を値のまま渡すと具体的なライフタイムに
    // 単相化され、`classify_codex_stage_entries` が要求する
    // `Fn(&Path) -> ...`(任意のライフタイムで呼べること)を満たせず HRTB
    // エラーになる。クロージャで包んで再ジェネリック化する。
    classify_codex_stage_entries(codex_dir, |path| std::fs::symlink_metadata(path))
}

/// `host_codex_stage_entries` の本体。stat 関数を注入できる形にして、
/// NotFound 以外の stat エラー(権限エラー等)が wipe を誘発しないことを
/// テストで固定できるようにする。
fn classify_codex_stage_entries<F>(
    codex_dir: &std::path::Path,
    stat: F,
) -> anyhow::Result<Vec<(std::path::PathBuf, &'static str)>>
where
    F: Fn(&std::path::Path) -> std::io::Result<std::fs::Metadata>,
{
    let mut entries = Vec::new();
    for name in HOST_CODEX_ALLOWLIST {
        let path = codex_dir.join(name);
        match stat(&path) {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_file() {
                    entries.push((path, *name));
                } else if file_type.is_symlink() {
                    eprintln!(
                        "warning: {} is a symlink; skipped (codex auth must be a regular file)",
                        path.display()
                    );
                }
                // ディレクトリ等、ファイルでも symlink でもない場合は
                // allowlist が想定しない形状のため黙ってスキップする。
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // config.toml が無いホストは通常運用。警告不要で黙ってスキップ。
            }
            Err(e) => {
                // パーミッション等 NotFound 以外の失敗は「存在しない」ではない
                // ため黙ってスキップせず、どのパスの stat に失敗したかを含めて
                // 呼び出し元へ伝播する(呼び出し元が wipe を誤発動させないため)。
                return Err(e).with_context(|| format!("Failed to stat {}", path.display()));
            }
        }
    }
    Ok(entries)
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
///
/// `location` は削除ログに出す人間可読なラベル(例: `"codex auth store"` /
/// `"codex stage"`)。この関数は auth store・ステージ双方の掃除に使われる
/// 共通処理だが、以前は常に「codex stage」固定文言でログ出力しており、
/// 実際には auth store を掃除している場合でも「stage」と表示され運用者が
/// 誤解する余地があった(Copilot レビュー指摘)。呼び出し元がどちらを
/// 掃除しているかを明示させることで、削除ログだけで対象を特定できるように
/// する。
fn reconcile_codex_stage_dir(
    dir: &std::path::Path,
    keep: &[&str],
    location: &str,
) -> anyhow::Result<()> {
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
        eprintln!("removed stale {location} entry not in allowlist: {name_str}");
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

/// **auth store**(`<config_dir>/codex-auth/`)への並行アクセスを直列化する
/// アドバイザリロック(codex レビュー round 7 P1、round 10 で対象を
/// per-container ステージから auth store に付け替え)。
///
/// per-container ステージ(`<runtime_dir>/codex/`)はコンテナ名で分離される
/// ため、並行 run 間でステージ自体が競合することはもう無い。しかし
/// auth store は `prepare_codex_mount`(run 前: host→store→stage)と
/// `sync_codex_stage_to_store`(run 後: stage→store)の両方から触られる
/// 全プロセス共有の単一ディレクトリであり、複数の `vibepod run` が同時に
/// これを操作すると、一方の完全リコンサイル(`reconcile_codex_stage_dir`)が
/// 他方が書き込み中の `NamedTempFile`(round 6 で一意名にした allowlist 外の
/// 一時ファイル)を「allowlist に無いエントリ」として削除してしまうことが
/// ある。このロックを両関数の本体全体の排他区間にすることで、複数呼び出しが
/// 同じ store をインターリーブして操作しないことを保証する。
///
/// UX は `BuildLock`(`src/cli/init.rs`)と同じパターンに倣う:
/// `flock(LOCK_EX | LOCK_NB)` を先に試し、競合(`EWOULDBLOCK`)のときだけ
/// 待機を告知してから `flock(LOCK_EX)` でブロッキング取得する。`BuildLock`
/// はビルド専用の UX 文言・ファイル名(`build.lock`)を持つ private struct
/// であり、無理に汎用化すると既存の init 経路の意味論に codex 側の事情
/// (ファイル名・文言)を持ち込むリスクの方が大きいため、独立実装とする
/// (spec が許容する選択)。
///
/// ロックファイルは auth store(`<config_dir>/codex-auth/`)の**外**、
/// `<config_dir>/codex.lock` に置く。store 内に置くと
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

/// `entries`(コピー元の一覧、`host_codex_stage_entries` が返す形式)を `dst_dir`
/// へ反映する共通コピーループ(codex レビュー round 10 で
/// `prepare_codex_mount` から抽出。旧実装は host→stage の 1 ホップのみを
/// このループでベタ書きしていたが、round 10 で host→store→stage の 2 ホップ
/// 構成になったため、同じ「reconcile → per-entry keep-newest/上書きコピー →
/// 権限強制」ロジックを 2 回呼べるようこの関数に切り出した)。
///
/// 処理内容(旧 `prepare_codex_mount` のループ本体と完全に同一のロジック):
/// 1. `dst_dir` を作成し 0700 にする
/// 2. `entries` に無い名前(あるいは allowlist 外の残置物)を全リコンサイル
///    (`reconcile_codex_stage_dir`)
/// 3. 各エントリについて、非通常ファイル(symlink・ディレクトリ)なら削除し
///    (`remove_dst_if_not_regular_file`)、`auth.json` は keep-newest
///    (`should_keep_staged_auth`)、`config.toml` は常にソース優先で
///    コピー(`copy_codex_asset_atomically`)
/// 4. 全エントリの権限を 0600 に強制(`enforce_staged_permissions`)
///
/// 呼び出し元(`prepare_codex_mount`)がロック(`CodexStageLock`)を保持した
/// 状態でこの関数を呼ぶことを前提とする(この関数自身はロックを取らない)。
///
/// `location` は内部で呼ぶ `reconcile_codex_stage_dir` にそのまま渡す削除
/// ログ用ラベル(`CODEX_AUTH_STORE_LOCATION_LABEL` / `CODEX_STAGE_LOCATION_LABEL`
/// のいずれか)。同じ場所を指す呼び出しは常に同じ文言になるよう、呼び出し元は
/// この2定数のみを使うこと。
fn sync_codex_entries_into(
    entries: &[(std::path::PathBuf, &'static str)],
    dst_dir: &std::path::Path,
    location: &str,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst_dir)
        .with_context(|| format!("Failed to create {}", dst_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dst_dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("Failed to set permissions on {}", dst_dir.display()))?;
    }

    // P2: 今回の entries に無い allowlist ファイル(例: ホストで config.toml が
    // 削除された)が dst に残っていると無期限に使われ続けるため、コピー前に
    // 差分を削除しておく。
    let keep_names: Vec<&str> = entries.iter().map(|(_, name)| *name).collect();
    reconcile_codex_stage_dir(dst_dir, &keep_names, location).with_context(|| {
        format!(
            "Failed to reconcile stale codex assets in {}",
            dst_dir.display()
        )
    })?;

    for (src, name) in entries {
        let dst = dst_dir.join(name);

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
                    // 古いソースコピーで上書きしない(round 3 P1)。コンテナが
                    // この経路でパーミッションだけ緩めていた場合の修復は、この
                    // ループを抜けた後の `enforce_staged_permissions` に集約する
                    // (round 9 P1)。
                    continue;
                }
            }
        }

        copy_codex_asset_atomically(src, &dst)?;
    }

    enforce_staged_permissions(dst_dir, entries)?;

    Ok(())
}

/// ホストの `~/.codex/auth.json`(と存在すれば `config.toml`)を、
/// **auth store**(`<config_dir>/codex-auth/`、host-only・コンテナには一切
/// マウントしない)を経由して **per-container ステージ**
/// (`<runtime_dir>/codex/`)へ反映し、そのステージのパスを返す。呼び出し元は
/// このパスをコンテナへ `/home/vibepod/.codex` として **rw** マウントする(codex が
/// トークンリフレッシュ時に auth.json を書き換えるため。コピーなのでホスト原本には
/// 影響しない — `.claude.json` と同じパターン)。
///
/// **新アーキテクチャ(round 10、「見せる領域」と「永続化する領域」の分離)**:
///
/// - コンテナに見せる領域は per-container ステージのみ。codex がステージへ
///   書き込む実行データ(history/sessions/cache 等)はこのコンテナ専用領域に
///   閉じ、他コンテナからは不可視。disposable 実行(`--new` / worktree)の
///   終了処理がステージごと `runtime_dir` を `remove_dir_all` しても、それは
///   単なるスナップショットの破棄であり問題ない(round 4 で懸念していた
///   「唯一の有効コピーが失われる」問題は、認証の永続化を auth store 側に
///   分離したことで解消した)。
/// - 永続化する領域は auth store のみで、**コンテナには一切マウントしない**。
///   ホストプロセス(この関数と `sync_codex_stage_to_store`)だけが読み書きする。
///
/// 同期は host → store → stage の順に 2 ホップで行う(`sync_codex_entries_into`
/// を host entries→store、store entries→stage の 2 回呼ぶ)。run 後にコンテナが
/// ステージ内の `auth.json` をリフレッシュした場合の store への書き戻しは、
/// コンテナ停止処理・cleanup 前に呼ばれる `sync_codex_stage_to_store` が担う
/// (強制終了(kill -9 等)でこの書き戻しが走らなかった場合、直近のリフレッシュは
/// 失われうるが、ホストで codex 再ログインすれば回復できる、accepted risk)。
///
/// `auth.json` が存在しない場合は `None` を返し、codex 注入をスキップする
/// (vibepod 自体は動作継続するが、コンテナ内で codex レビューは使えない)。
/// エラーの握りつぶし禁止のルールに従い、この場合は stderr に理由を明示する。
/// このとき store・ステージ双方の残置物を全消去する(過去の run でどちらかに
/// 残っていた認証情報が、既存コンテナの bind mount 経由で使われ続けたり、
/// 次回 store→stage 同期で復活したりしないようにするため)。
///
/// `config.toml` のみ無く `auth.json` はある場合は、警告なしで auth.json だけ
/// コピーして続行する(`config.toml` 省略は codex のデフォルト設定を使う正常な
/// 運用パターンのため)。
///
/// **`auth.json` は「新しい方を保持」する**(codex レビュー round 3 P1、両ホップに
/// 適用): ステージ(または store)済み `auth.json` の mtime がコピー元より新しければ、
/// コンテナ内 codex がリフレッシュしたものとみなしコピーをスキップする
/// (`should_keep_staged_auth` 参照)。`config.toml` は認証情報ではなく設定ファイルの
/// ため、この特例の対象外であり常にホスト側で上書きする。
///
/// 関数本体全体(リコンサイル開始〜全コピー完了、両ホップとも)は
/// `CodexStageLock`(`<config_dir>/codex.lock`)による排他区間内で実行される
/// (codex レビュー round 7 P1、round 10 で対象を auth store に付け替え)。
/// store は `prepare_codex_mount` と `sync_codex_stage_to_store` の双方から
/// 触られるため、両方をこのロックで直列化する。
///
/// 削除ログ(`reconcile_codex_stage_dir`)の対象ラベル。auth store
/// (`<config_dir>/codex-auth/`)とステージ(`<runtime_dir>/codex/`)は削除の
/// 意味が異なる(前者は永続化領域の掃除、後者は使い捨て領域の掃除)ため、
/// この関数内のすべての呼び出しをこの2定数のどちらかに統一し、同じ場所を
/// 指す呼び出しが常に同じ文言でログに出るようにする(Copilot レビュー指摘、
/// PR #56)。
const CODEX_AUTH_STORE_LOCATION_LABEL: &str = "codex auth store";
const CODEX_STAGE_LOCATION_LABEL: &str = "codex stage";

pub fn prepare_codex_mount(
    home: &std::path::Path,
    config_dir: &std::path::Path,
    runtime_dir: &std::path::Path,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    // round 7 P1 / round 10: リコンサイル開始から全コピー完了まで(この関数の
    // 残り全体、early return を含むすべての return パス、host→store と
    // store→stage の両ホップ)を排他区間にする。ロックガードは関数を抜ける
    // とき(どの return パスでも)に drop され、解放される。
    let _stage_lock = acquire_codex_stage_lock(config_dir)?;

    let host_codex_dir = home.join(".codex");
    let host_entries = host_codex_stage_entries(&host_codex_dir).with_context(|| {
        format!(
            "Failed to enumerate host codex directory {}",
            host_codex_dir.display()
        )
    })?;

    let store_dir = config_dir.join("codex-auth");
    let stage_dir = runtime_dir.join("codex");

    let has_auth = host_entries.iter().any(|(_, name)| *name == "auth.json");
    if !has_auth {
        // P1: ホストの auth.json が無い(未認証 or 取り消し済み)。過去の run で
        // store・ステージいずれかに残っている認証情報が、既存コンテナの
        // bind mount 経由で使われ続けたり、次回同期で復活したりしないよう、
        // ディレクトリ自体は残したまま中身だけ両方とも全消去する(keep が空 =
        // allowlist 全ファイルが削除対象)。
        reconcile_codex_stage_dir(&store_dir, &[], CODEX_AUTH_STORE_LOCATION_LABEL).with_context(
            || {
                format!(
                    "Failed to clear stale codex assets in auth store {}",
                    store_dir.display()
                )
            },
        )?;
        reconcile_codex_stage_dir(&stage_dir, &[], CODEX_STAGE_LOCATION_LABEL).with_context(
            || {
                format!(
                    "Failed to clear stale codex assets in stage {}",
                    stage_dir.display()
                )
            },
        )?;
        eprintln!(
            "codex auth not found (~/.codex/auth.json); codex review is unavailable in this container"
        );
        return Ok(None);
    }

    // Hop 1: host -> store.
    sync_codex_entries_into(&host_entries, &store_dir, CODEX_AUTH_STORE_LOCATION_LABEL)
        .with_context(|| format!("Failed to sync codex auth store {}", store_dir.display()))?;

    // Hop 2: store -> stage. 直前の hop で store には必ず auth.json が入って
    // いるので、store 側の実体を再列挙してそのままステージへ反映する。
    let store_entries = host_codex_stage_entries(&store_dir).with_context(|| {
        format!(
            "Failed to enumerate codex auth store {}",
            store_dir.display()
        )
    })?;
    sync_codex_entries_into(&store_entries, &stage_dir, CODEX_STAGE_LOCATION_LABEL)
        .with_context(|| format!("Failed to sync codex stage {}", stage_dir.display()))?;

    Ok(Some(stage_dir))
}

/// per-container ステージ(`<runtime_dir>/codex/`)の `auth.json` を、
/// **auth store**(`<config_dir>/codex-auth/`)へ同期する(codex レビュー
/// round 10、run 後同期)。
///
/// コンテナ内 codex はステージ(rw マウント経由)の `auth.json` をトークン
/// リフレッシュ時に書き換えることがある。ステージは disposable cleanup
/// (`runtime_dir` の `remove_dir_all`)で消える前提の使い捨て領域なので、
/// リフレッシュされたトークンを永続化する唯一の経路が、コンテナ停止後・
/// cleanup 前にこの関数を呼んでステージから store へ書き戻すことである。
///
/// 呼び出し順は `interactive.rs` / `prompt.rs` の両方で、コンテナ exec 完了後、
/// disposable/non-disposable いずれの cleanup 分岐(disposable なら
/// `remove_dir_all(&ctx.runtime_dir)` でステージごと削除、non-disposable なら
/// `docker stop`)よりも**前**でなければならない。cleanup が先に走ると、
/// 同期元のステージ自体が消えてしまう。
///
/// `config.toml` はコンテナが一切書き換えないファイルなので、ここでは
/// `auth.json` のみを扱う。
///
/// **TOCTOU 対策(codex レビュー round 10 差し戻し、Critical)**: この関数の
/// コピー元(`stage_auth`)は、呼び出し時点でまだ生きているコンテナが rw
/// マウント経由でいつでも書き換えられる untrusted な入力である —
/// `prepare_codex_mount` 側の各種防御はすべて「dst(ステージ/store)が
/// untrusted、src(ホストの `~/.codex/` または host-only store)は信頼できる」
/// 前提だったが、この関数は逆に **src がコンテナ制御下にある**。型チェック
/// (symlink 判定)・mtime 取得・内容読み取りを別々の path 解決で行うと、
/// チェックと読み取りの間の窓でコンテナが `stage_auth` をホスト任意パス
/// (例 `~/.ssh/id_rsa`)への symlink に差し替え、その内容が store に
/// 取り込まれてしまう(次回 run で当該コンテナへ再マウントされ漏洩する)。
/// これを防ぐため、`sync_codex_stage_auth_via_fd`(unix 実装)が
/// `stage_auth` を `O_NOFOLLOW | O_NONBLOCK` で一度だけ開き、以降の
/// fstat(mtime・file_type)・内容読み取りをすべて同一 fd 経由で行う。open
/// 自体が symlink なら `ELOOP` で失敗するため、「symlink でないことの確認」
/// と「その後の読み取り」が同一 fd で不可分になり、開いた後にコンテナが
/// symlink へ差し替えてもこの fd は元の inode(通常ファイル)を指し続ける —
/// チェックと act が同一 fd で行われることで TOCTOU の窓が構造的に消える
/// (`enforce_staged_permissions` が dst の chmod に対して行っている防御と
/// 同型を、こちらは src の read に適用したもの)。
///
/// 失敗時は `.with_context` 付きで呼び出し元へ伝播する。呼び出し元
/// (`interactive.rs` / `prompt.rs`)は、この同期の失敗で成功した run 自体を
/// 失敗扱いにしたり cleanup をブロックしたりしない方針のため、エラーを
/// stderr に warning として出力してから通常の cleanup を継続する(エラー
/// パスの握りつぶし禁止ルールに従い、`.ok()` で無言に捨てはしない)。
pub fn sync_codex_stage_to_store(
    runtime_dir: &std::path::Path,
    config_dir: &std::path::Path,
) -> anyhow::Result<()> {
    // store は prepare_codex_mount と同じロックで直列化する(auth store は
    // 両関数から触られる全プロセス共有のディレクトリのため)。
    let _stage_lock = acquire_codex_stage_lock(config_dir)?;

    let stage_auth = runtime_dir.join("codex").join("auth.json");
    let store_dir = config_dir.join("codex-auth");
    let store_auth = store_dir.join("auth.json");

    #[cfg(unix)]
    {
        sync_codex_stage_auth_via_fd(&stage_auth, &store_dir, &store_auth)
    }
    #[cfg(not(unix))]
    {
        // 非 unix 環境では symlink 検出込みのアトミックな open(O_NOFOLLOW)
        // が使えないため、path ベースの二段階チェックに留める。本
        // プロジェクトは事実上 unix(Docker Desktop / OrbStack on
        // macOS・Linux)専用であり、他の TOCTOU 対策
        // (`enforce_staged_permissions` の fd chmod、`CodexStageLock` の
        // flock)も同様に unix 限定であるため、既存の方針に合わせる。
        remove_dst_if_not_regular_file(&stage_auth)?;
        let stage_meta = match std::fs::symlink_metadata(&stage_auth) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to stat {}", stage_auth.display()))
            }
        };
        let stage_mtime = stage_meta
            .modified()
            .with_context(|| format!("Failed to read mtime of {}", stage_auth.display()))?;

        let store_mtime = match std::fs::symlink_metadata(&store_auth) {
            Ok(meta) if meta.file_type().is_file() => Some(
                meta.modified()
                    .with_context(|| format!("Failed to read mtime of {}", store_auth.display()))?,
            ),
            _ => None,
        };
        // store 側の既存コピーは、ステージ側が厳密に新しい場合だけ上書きする
        // (run 後同期の意図。mtime 同値なら store を保持=コピーしない)。
        let should_copy = store_mtime.is_none_or(|m| stage_mtime > m);
        if !should_copy {
            return Ok(());
        }

        // round 11 P1 / round 12 P1-a: ステージの auth.json はコンテナが rw
        // マウント経由で書き込む untrusted な入力である。コンテナ稼働中の torn
        // (途中)書き込みだけでなく、`docker stop` が SIGKILL に落ちた場合の
        // truncated な残骸も store に取り込むと壊れた認証が全コンテナへ配布
        // される。そのため liveness に関わらず**常に**「完全な JSON として
        // パースできるか」を反映前の追加ゲートにする。パース不能なら store に
        // 一切触れずスキップし(既存コピーの内容・mtime を保ち、無ければ作らない)、
        // 次回 run で再試行させる。unix 実装(fd 経由)と挙動を揃える。
        let contents = std::fs::read(&stage_auth)
            .with_context(|| format!("Failed to read staged {} for sync", stage_auth.display()))?;
        if serde_json::from_slice::<serde_json::Value>(&contents).is_err() {
            eprintln!(
                "stage auth.json was not valid JSON; skipped store sync (possibly mid-write)"
            );
            return Ok(());
        }

        std::fs::create_dir_all(&store_dir)
            .with_context(|| format!("Failed to create auth store {}", store_dir.display()))?;
        copy_codex_asset_atomically(&stage_auth, &store_auth)?;
        Ok(())
    }
}

/// `sync_codex_stage_to_store` の unix 実装本体。`stage_auth`(コンテナ制御下、
/// untrusted)を `O_NOFOLLOW | O_NONBLOCK` で一度だけ開き、以降の型・mtime
/// 判定とコピー元内容の読み取りをすべて同一 fd から行うことで、開いた後に
/// コンテナが `stage_auth` を symlink へ差し替える TOCTOU を構造的に無効化
/// する(詳細は `sync_codex_stage_to_store` のドキュメント参照)。
#[cfg(unix)]
fn sync_codex_stage_auth_via_fd(
    stage_auth: &std::path::Path,
    store_dir: &std::path::Path,
    store_auth: &std::path::Path,
) -> anyhow::Result<()> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(stage_auth)
    {
        Ok(file) => file,
        // ステージに auth.json が元々無かった(codex 未使用のコンテナ)。
        // 同期対象が無いだけの正常系であり、store には一切触れない。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        // コンテナが symlink に差し替えていた場合の open(2) の挙動。辿らず
        // 検出できた時点でこの防御の目的は達成しているため、内容には一切
        // 触れずに削除して警告する(store 無傷を保証)。実際の削除処理は
        // `remove_dst_if_not_regular_file`(symlink_metadata による再判定を
        // 含む)に委ねる — 削除は内容を読まない操作なので、この委譲で新たな
        // TOCTOU は生じない。
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            eprintln!(
                "staged codex auth.json was a symlink at sync time (possible container \
                 tampering); removed without following it, auth store left untouched: {}",
                stage_auth.display()
            );
            remove_dst_if_not_regular_file(stage_auth)?;
            return Ok(());
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to open {} for sync", stage_auth.display()))
        }
    };

    let meta = file.metadata().with_context(|| {
        format!(
            "Failed to stat opened fd for {} before sync",
            stage_auth.display()
        )
    })?;

    if !meta.file_type().is_file() {
        // O_NOFOLLOW は symlink を弾くが、ディレクトリ等の他の非通常ファイル
        // はそのまま開けてしまう。symlink と同じ方針で、内容には触れず削除
        // して警告する。
        drop(file);
        eprintln!(
            "staged codex auth.json was not a regular file at sync time (possible container \
             tampering); removed, auth store left untouched: {}",
            stage_auth.display()
        );
        remove_dst_if_not_regular_file(stage_auth)?;
        return Ok(());
    }

    // ここまでで stage_auth は「open した fd の時点で」通常ファイルである
    // ことが保証されている。以降の mtime・内容取得は path の再解決を一切
    // 行わず、この fd(またはこの fd から読み取ったバイト列)のみを使う。
    let stage_mtime = meta
        .modified()
        .with_context(|| format!("Failed to read mtime of {}", stage_auth.display()))?;

    let store_mtime = match std::fs::symlink_metadata(store_auth) {
        Ok(meta) if meta.file_type().is_file() => Some(
            meta.modified()
                .with_context(|| format!("Failed to read mtime of {}", store_auth.display()))?,
        ),
        // store 側が存在しない、または(あり得ないはずだが)通常ファイルで
        // ない場合は「store には有効な既存コピーが無い」として扱い、必ず
        // コピーする。store は host-only なので、この分岐がコンテナ由来の
        // 改ざんを表すことはない。
        _ => None,
    };

    // store 側の既存コピーは、ステージ側が厳密に新しい場合だけ上書きする
    // (run 後同期の意図。mtime 同値なら store を保持=コピーしない)。
    let should_copy = store_mtime.is_none_or(|m| stage_mtime > m);

    if !should_copy {
        return Ok(());
    }

    // 内容もこの時点で同一 fd から読む(symlink 判定に使った fd をそのまま
    // 使い回すため、path 経由の再オープンを一切行わない — これが本関数の
    // TOCTOU 対策の核心: 「symlink でない」という判定と「その内容を読む」
    // 行為が不可分になる)。
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .with_context(|| format!("Failed to read staged {} for sync", stage_auth.display()))?;

    // round 11 P1 / round 12 P1-a: ここで読んだスナップショットは、コンテナが
    // 稼働継続中なら書き込み途中(torn)である可能性があり、`docker stop` が
    // SIGKILL に落ちた停止済みコンテナでも truncated な残骸である可能性がある。
    // 不完全な内容を store に取り込むと壊れた認証が全コンテナへ配布されるため、
    // store へ書き込む temp ファイルを作る前に liveness に関わらず**常に**
    // 「完全な JSON か」を追加ゲートとして検証する。パース不能なら store には
    // 一切触れず(既存コピーの内容・mtime を保ち、無ければ作らない)、警告を
    // 出してスキップする — 次回 run で再試行される。
    if serde_json::from_slice::<serde_json::Value>(&contents).is_err() {
        eprintln!("stage auth.json was not valid JSON; skipped store sync (possibly mid-write)");
        return Ok(());
    }

    std::fs::create_dir_all(store_dir)
        .with_context(|| format!("Failed to create auth store {}", store_dir.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(store_dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("Failed to set permissions on {}", store_dir.display()))?;
    }

    // 読み取った内容を、store ディレクトリ内に新規作成した一意名の一時
    // ファイルへいったん書き出す。このファイルは今この関数が作ったばかりで
    // 名前を事前に知りようがなく、かつ store_dir はどのコンテナにも
    // マウントされないため、コンテナがこれを差し替える余地はない(=信頼
    // できる src)。その上で `copy_codex_asset_atomically` の既存のアトミック
    // 置換・0600 強制・dst(store)側 tamper 検査ロジックをそのまま再利用して
    // store へ反映する — この関数の役割は「untrusted な src を安全に読む」
    // ことに専念させ、「trusted な内容を安全に dst へ書く」ロジックは
    // 重複実装せず既存の監査済みコードに委ねる。
    let content_tmp = tempfile::NamedTempFile::new_in(store_dir).with_context(|| {
        format!(
            "Failed to create a unique temp file in {} for syncing {}",
            store_dir.display(),
            store_auth.display()
        )
    })?;
    std::fs::write(content_tmp.path(), &contents).with_context(|| {
        format!(
            "Failed to write staged content into temp file {}",
            content_tmp.path().display()
        )
    })?;

    copy_codex_asset_atomically(content_tmp.path(), store_auth)?;
    // copy_codex_asset_atomically は実際にコピーを行った経路では必ず 0600
    // を設定するが、store の内容が既に byte-for-byte 一致する場合は早期
    // リターンして chmod を行わない(round 8 P2-2)。store は host-only で
    // 通常はこれ以前に権限が緩む理由が無いが、「経路によらず必ず 0600」と
    // いう round 9 の原則を dst=store 側にも一貫して適用するため、ここでも
    // 念のため強制する。
    enforce_staged_permissions(store_dir, &[(store_auth.to_path_buf(), "auth.json")])?;

    Ok(())
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

/// codex ステージ(rw マウント経由でコンテナがリフレッシュした可能性がある
/// auth.json)を host-only の auth store へ書き戻す。`interactive.rs` /
/// `prompt.rs` の runtime dir を保持する cleanup 分岐(非 disposable で
/// コンテナを停止する経路、および元から Running のまま切断する経路)から呼ぶ。
///
/// disposable(`docker rm -f` でコンテナごと runtime dir を捨てる)経路は、
/// 後片付けの削除可否まで含めて判断する `finalize_disposable_runtime_dir` を
/// 使う(この関数はステージを保持したまま同期のみを行う)。
///
/// 同期は liveness に関わらず auth.json の JSON 完全性を検証してから行い
/// (`sync_codex_stage_to_store`)、書き込み途中や truncated で不完全な
/// スナップショットは取り込まずスキップする(round 11 P1 / round 12 P1-a)。
///
/// 同期の失敗は成功した run 自体を失敗にしないが、握りつぶさず stderr に
/// 警告する。
pub(super) fn sync_codex_stage_after_run(ctx: &RunContext) {
    if ctx.codex_dir.is_some() {
        if let Err(e) = sync_codex_stage_to_store(&ctx.runtime_dir, &ctx.config_dir) {
            eprintln!("warning: failed to sync codex auth back to host store: {e:#}");
        }
    }
}

/// disposable コンテナの後片付けコア。`liveness` は docker rm -f の exit status
/// から呼び出し側が導出する: 削除成功なら Stopped、失敗(コンテナが生存し得る)
/// なら Running。codex ステージは liveness に関わらず常に(検証付きで)store へ
/// 同期する。Stopped のときだけステージごと runtime dir を削除し、Running では
/// コンテナがまだステージに書き込み得るため runtime dir を保持して次回 run に
/// 委ねる(round 12 P1-b)。
pub fn finalize_disposable_runtime_dir(
    runtime_dir: &std::path::Path,
    config_dir: &std::path::Path,
    codex_present: bool,
    liveness: ContainerLiveness,
) {
    if codex_present {
        if let Err(e) = sync_codex_stage_to_store(runtime_dir, config_dir) {
            eprintln!("warning: failed to sync codex auth back to host store: {e:#}");
        }
    }
    match liveness {
        ContainerLiveness::Stopped => {
            if let Err(e) = std::fs::remove_dir_all(runtime_dir) {
                // 削除失敗を捨てると、auth.json を含む runtime dir が残置
                // されたまま「後片付け完了」と誤認されてしまう(codex レビュー
                // round 13 P2)。fatal にはしないが、運用者が残置を検知して
                // 手動対処できるよう stderr に警告する。
                eprintln!(
                    "warning: failed to remove runtime dir {}: {e}; manual cleanup required: rm -rf {}",
                    runtime_dir.display(),
                    runtime_dir.display()
                );
            }
        }
        ContainerLiveness::Running => {
            // コンテナ削除に失敗して稼働継続の可能性があるため、ステージを含む
            // runtime dir は削除せず保持する。失敗内容と手動対処は呼び出し側が
            // 既に stderr に出している。
        }
    }
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
        rw_mounts: ctx.rw_mounts.clone(),
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

/// `build_config_labels` が `vibepod.mounts` ラベルへ渡す `mount_parts` を
/// 組み立てる純関数。
///
/// `prepare.rs` 9b（既存コンテナとの設定差分検知）が独立に組み立てる
/// `mounts_parts` と**完全に同じ文字列表現**を生成する必要がある。ここが
/// ずれると、9b の比較が常に不一致になり、コンテナを再作成した直後でも
/// 「設定が変更されました」という警告が永久に出続ける（実際に round 2 で
/// 見つかった不具合の再発パターン）。9b 側は実パスの代わりにマーカーで
/// 表現する箇所が2つあるため、こちらも同じ2箇所をマーカーへ置換／追加する:
///
/// - **sanitized settings**: `extra_mounts` 内のエントリのうち、ホスト側
///   パスが `sanitized_settings_host`（= `prepare_sanitized_settings_mount`
///   が書き出す実パス、通常 `<config_dir>/runtime/<container_name>/settings.json`）
///   と一致し、**かつ**コンテナ側パスが `SANITIZED_SETTINGS_CONTAINER_PATH`
///   であるものだけを `SANITIZED_SETTINGS_LABEL_MARKER` へ**置換**する
///   （追加ではない — 元の実パス文字列は残さない）。コンテナ側パスだけで
///   判定してはいけない: ユーザーが `--mount /foo:/home/vibepod/.claude/settings.json`
///   を指定した場合に誤って置換され、設定変更の検知がマスクされてしまう
///   （`prepare.rs` の `warn_config_changes` 内コメントが警告している既存の
///   懸念と同じ理由）。
/// - **plugins/data**: `rw_mounts` の中にコンテナ側パスが
///   `DEFAULT_PLUGINS_DATA_CONTAINER_PATH` のエントリが1つ以上あれば、
///   `PLUGINS_DATA_MOUNT_LABEL_MARKER` を**1つだけ** push する。`rw_mounts`
///   はホスト HOME が `/home/vibepod` でない場合 `plugins_data_mount_entries`
///   により2要素になるが、9b 側は `plugins_data_stage.is_some()` の有無
///   だけで1つしか push しないため、要素数に関わらずマーカーは1つに揃える。
///   `rw_mounts` の生の `host:container` 自体は `mount_parts` に含めない
///   （ホスト側パスがコンテナ名に依存する per-container ステージのパスで、
///   9b 側はマーカーしか持たないため一致しなくなる）。
fn mounts_label_parts(
    extra_mounts: &[(String, String)],
    rw_mounts: &[(String, String)],
    sanitized_settings_host: Option<&std::path::Path>,
) -> Vec<String> {
    let mut parts: Vec<String> = extra_mounts
        .iter()
        .map(|(host, container)| {
            let is_sanitized_settings = container == SANITIZED_SETTINGS_CONTAINER_PATH
                && sanitized_settings_host
                    .map(|expected| expected == std::path::Path::new(host.as_str()))
                    .unwrap_or(false);
            if is_sanitized_settings {
                SANITIZED_SETTINGS_LABEL_MARKER.to_string()
            } else {
                format!("{}:{}", host, container)
            }
        })
        .collect();

    if rw_mounts
        .iter()
        .any(|(_, container)| container == DEFAULT_PLUGINS_DATA_CONTAINER_PATH)
    {
        parts.push(PLUGINS_DATA_MOUNT_LABEL_MARKER.to_string());
    }

    parts
}

/// コンテナのラベルを生成する（設定変更の検知に使用）。
pub(super) fn build_config_labels(ctx: &RunContext) -> std::collections::HashMap<String, String> {
    let mut labels = std::collections::HashMap::new();

    // マウントパスをソートして結合。sanitized settings / plugins-data の
    // マーカー変換は `mounts_label_parts` に集約している（`prepare.rs` 9b
    // との表現一致を保証するため、詳細はそちらの doc コメント参照）。
    let sanitized_settings_host = ctx
        .config_dir
        .join("runtime")
        .join(&ctx.container_name)
        .join("settings.json");
    let mount_parts = mounts_label_parts(
        &ctx.extra_mounts,
        &ctx.rw_mounts,
        Some(sanitized_settings_host.as_path()),
    );
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

    // profile: 未指定は空文字列で保存する（prepare.rs の 9b 比較ロジックと
    // 同じ表現に揃える）。値が変わった場合の再作成促しは他ラベルと同一の
    // warn_config_changes 経路に乗る。
    labels.insert(
        "vibepod.profile".to_string(),
        ctx.profile.clone().unwrap_or_default(),
    );

    // Label schema version. Monotonically increasing: never decrement,
    // even when features are removed (a smaller value would misidentify
    // newer containers as older ones). Previous releases used "1" (legacy
    // single-token `vibepod.lang`), "2" (full comma-joined lang set), "3"
    // (added the now-removed `vibepod.template_setup_hash`), and "4"
    // (removed the template machinery). Adding `vibepod.profile` advances
    // to "5".
    //
    // Currently this value is NOT read anywhere — no code branches on it.
    // It is written and reserved for a future backward-compatibility gate
    // that may need to distinguish container schema generations.
    labels.insert("vibepod.labels_version".to_string(), "5".to_string());

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

/// `--prompt-file <path>` の内容を読み込み、検証する純関数。
///
/// 要件3（設計書 第4節）: ホストシェルの解釈を経由せずプロンプトを渡すための
/// 経路。内容は**無加工**（trim せず）で返す — 山括弧・波括弧・バッククォート・
/// `$` を含むファイルでも、そのまま `claude -p` へ渡る `opts.prompt` になる
/// 必要があるため。
///
/// エラーは運用者がすぐ直せるよう、常にパスを含める:
/// - ファイルが読めない場合（存在しない・権限がない等）
/// - 内容が空、または空白のみの場合（`claude -p ""` を渡すのは無意味なため
///   起動前に弾く）
pub fn resolve_prompt_file(path: &str) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read prompt file: {}", path))?;
    if content.trim().is_empty() {
        anyhow::bail!("Prompt file is empty or whitespace-only: {}", path);
    }
    Ok(content)
}

pub async fn execute(opts: RunOptions) -> Result<()> {
    // clap の `conflicts_with = "prompt"` により `--prompt` と `--prompt-file`
    // の同時指定は既に弾かれているため、ここで両方 Some になるケースは無い。
    // 読み込み内容は無加工で opts.prompt へ代入することで、以降の全経路
    // （ロック・タイムアウト・`claude -p` への受け渡し・ログ・サマリ）を
    // `--prompt` と完全に同一にする。
    let mut opts = opts;
    if let Some(ref path) = opts.prompt_file {
        opts.prompt = Some(resolve_prompt_file(path)?);
    }

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

    // F9（フル再レビュー指摘）: `format_timeout_duration` の端数表示回帰。

    #[test]
    fn format_timeout_duration_under_a_minute_uses_seconds() {
        assert_eq!(format_timeout_duration(45), "45 秒");
    }

    #[test]
    fn format_timeout_duration_exact_minutes_omits_seconds() {
        assert_eq!(format_timeout_duration(60), "1 分");
        assert_eq!(format_timeout_duration(1800), "30 分");
    }

    #[test]
    fn format_timeout_duration_with_remainder_shows_both_units() {
        // 旧実装は `90 / 60 == 1` で「1 分」に切り捨て、30 秒の端数を失っていた。
        assert_eq!(format_timeout_duration(90), "1 分 30 秒");
    }

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

    // --- mounts_label_parts: build_config_labels（書き込み側）が生成する
    // mount_parts が、prepare.rs 9b（比較側）と同じ表現になることを保証する
    // 純関数のテスト。round 2 で見つかった「二箇所が独立実装されてズレる」
    // 不具合の再発防止。

    #[test]
    fn mounts_label_parts_replaces_sanitized_settings_real_path_with_marker() {
        let sanitized_settings_host =
            std::path::PathBuf::from("/config/runtime/vibepod-proj-abc/settings.json");
        let extra_mounts = vec![(
            sanitized_settings_host.to_string_lossy().to_string(),
            "/home/vibepod/.claude/settings.json".to_string(),
        )];

        let parts = mounts_label_parts(&extra_mounts, &[], Some(sanitized_settings_host.as_path()));

        assert_eq!(parts, vec![SANITIZED_SETTINGS_LABEL_MARKER.to_string()]);
        assert!(
            !parts.iter().any(|p| p.contains("/config/runtime")),
            "the real runtime-dir host path must not leak into the label: {:?}",
            parts
        );
    }

    #[test]
    fn mounts_label_parts_does_not_replace_user_specified_mount_to_same_container_path() {
        // ユーザーが `--mount /foo:/home/vibepod/.claude/settings.json` を
        // 指定した場合、コンテナ側パスだけが一致してもホスト側パスが
        // sanitized settings の実パスと異なるため置換されてはならない
        // （置換すると設定変更の検知がマスクされる）。
        let sanitized_settings_host =
            std::path::PathBuf::from("/config/runtime/vibepod-proj-abc/settings.json");
        let extra_mounts = vec![(
            "/foo".to_string(),
            "/home/vibepod/.claude/settings.json".to_string(),
        )];

        let parts = mounts_label_parts(&extra_mounts, &[], Some(sanitized_settings_host.as_path()));

        assert_eq!(
            parts,
            vec!["/foo:/home/vibepod/.claude/settings.json".to_string()]
        );
    }

    #[test]
    fn mounts_label_parts_pushes_single_marker_for_multi_entry_rw_mounts() {
        // ホスト HOME が /home/vibepod でない場合、plugins_data_mount_entries
        // は2要素返すが、マーカーは1つだけ push される（9b 側が
        // plugins_data_stage.is_some() の有無だけで1つしか push しないため）。
        let rw_mounts = vec![
            (
                "/config/runtime/vibepod-proj-abc/plugins-data".to_string(),
                "/home/vibepod/.claude/plugins/data".to_string(),
            ),
            (
                "/config/runtime/vibepod-proj-abc/plugins-data".to_string(),
                "/Users/alice/.claude/plugins/data".to_string(),
            ),
        ];

        let parts = mounts_label_parts(&[], &rw_mounts, None);

        assert_eq!(parts, vec![PLUGINS_DATA_MOUNT_LABEL_MARKER.to_string()]);
        assert!(
            !parts.iter().any(|p| p.contains("plugins-data")),
            "the raw rw_mounts host:container tuple must not leak into the label: {:?}",
            parts
        );
    }

    #[test]
    fn mounts_label_parts_no_marker_when_rw_mounts_empty() {
        let parts = mounts_label_parts(&[], &[], None);
        assert!(
            !parts.contains(&PLUGINS_DATA_MOUNT_LABEL_MARKER.to_string()),
            "no plugins/data marker should appear when rw_mounts is empty: {:?}",
            parts
        );
        assert!(parts.is_empty());
    }

    #[test]
    fn mounts_label_parts_matches_prepare_context_9b_construction() {
        // 最重要: build_config_labels（mounts_label_parts 経由）が生成する
        // vibepod.mounts と、prepare.rs 9b が独立に組み立てる vibepod.mounts
        // が、同一の実行状態に対して完全一致することを確認する。9b はインライン
        // 実装で直接呼べないため、ここで同じ手順を再現する
        // （opts.mount → claude_config_mounts → SANITIZED marker →
        // PLUGINS_DATA marker → build_mounts_label）。
        let config_dir = std::path::PathBuf::from("/config");
        let container_name = "vibepod-proj-abc";
        let home = std::path::PathBuf::from("/Users/alice");
        let opts_mount_entries = vec![(
            "/host/user-mount".to_string(),
            "/container/user-mount".to_string(),
        )];
        let claude_config_mounts = vec![(
            "/Users/alice/.claude/CLAUDE.md".to_string(),
            "/home/vibepod/.claude/CLAUDE.md".to_string(),
        )];
        let host_settings_exists = true;
        let plugins_data_stage = Some("/config/runtime/vibepod-proj-abc/plugins-data".to_string());
        let host_codex_auth_exists = true;

        // --- 9b 相当（比較側）の再現 ---
        let mut mounts_parts_9b: Vec<String> = Vec::new();
        for (h, c) in &opts_mount_entries {
            mounts_parts_9b.push(format!("{}:{}", h, c));
        }
        for (h, c) in &claude_config_mounts {
            mounts_parts_9b.push(format!("{}:{}", h, c));
        }
        if host_settings_exists {
            mounts_parts_9b.push(SANITIZED_SETTINGS_LABEL_MARKER.to_string());
        }
        if plugins_data_stage.is_some() {
            mounts_parts_9b.push(PLUGINS_DATA_MOUNT_LABEL_MARKER.to_string());
        }
        let label_9b = build_mounts_label(mounts_parts_9b, host_codex_auth_exists);

        // --- build_config_labels 相当（書き込み側）の再現 ---
        let sanitized_settings_host = config_dir
            .join("runtime")
            .join(container_name)
            .join("settings.json");
        let mut extra_mounts: Vec<(String, String)> = Vec::new();
        extra_mounts.extend(opts_mount_entries);
        extra_mounts.extend(claude_config_mounts);
        if host_settings_exists {
            extra_mounts.push((
                sanitized_settings_host.to_string_lossy().to_string(),
                "/home/vibepod/.claude/settings.json".to_string(),
            ));
        }
        let rw_mounts = match &plugins_data_stage {
            Some(stage) => plugins_data_mount_entries(stage, &home),
            None => Vec::new(),
        };
        let parts_write_side = mounts_label_parts(
            &extra_mounts,
            &rw_mounts,
            Some(sanitized_settings_host.as_path()),
        );
        let label_write_side = build_mounts_label(parts_write_side, host_codex_auth_exists);

        assert_eq!(
            label_9b, label_write_side,
            "build_config_labels and prepare.rs 9b must produce identical vibepod.mounts \
             labels for the same run state, otherwise a freshly created container is \
             immediately flagged as configuration-changed"
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

    #[cfg(unix)]
    #[test]
    fn host_codex_stage_entries_excludes_symlinked_auth_json() {
        // codex レビュー round 13 P1: `auth.json` がホスト上で別ファイルへの
        // symlink になっている場合(dotfiles 管理ツール等で起こり得る)、
        // `is_file()` はリンクを辿ってしまいリンク先の内容が store/コンテナへ
        // 流れてしまう。symlink_metadata ベースの判定でこれを除外することを
        // 固定する。
        let codex_dir = tempfile::tempdir().expect("failed to create codex_dir tempdir");
        let target_dir = tempfile::tempdir().expect("failed to create symlink target tempdir");

        let target = target_dir.path().join("real_auth.json");
        std::fs::write(&target, r#"{"token":"LINKED_AWAY"}"#)
            .expect("failed to write symlink target");

        let auth_path = codex_dir.path().join("auth.json");
        std::os::unix::fs::symlink(&target, &auth_path)
            .expect("failed to create auth.json symlink");

        let entries = host_codex_stage_entries(codex_dir.path())
            .expect("stat of a plain tempdir must not fail");

        assert!(
            !entries.iter().any(|(_, name)| *name == "auth.json"),
            "a symlinked auth.json must be excluded from stage entries, got: {entries:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn host_codex_stage_entries_excludes_symlinked_config_toml() {
        // 上と同じ懸念を config.toml 側でも固定する。
        let codex_dir = tempfile::tempdir().expect("failed to create codex_dir tempdir");
        let target_dir = tempfile::tempdir().expect("failed to create symlink target tempdir");

        let target = target_dir.path().join("real_config.toml");
        std::fs::write(&target, "model = \"linked-away\"").expect("failed to write symlink target");

        let config_path = codex_dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &config_path)
            .expect("failed to create config.toml symlink");

        let entries = host_codex_stage_entries(codex_dir.path())
            .expect("stat of a plain tempdir must not fail");

        assert!(
            !entries.iter().any(|(_, name)| *name == "config.toml"),
            "a symlinked config.toml must be excluded from stage entries, got: {entries:?}"
        );
    }

    #[test]
    fn host_codex_stage_entries_includes_regular_files() {
        // 回帰確認: symlink 拒否を入れても、通常ファイルの auth.json /
        // config.toml は従来どおりエントリに含まれること。
        let codex_dir = tempfile::tempdir().expect("failed to create codex_dir tempdir");
        std::fs::write(codex_dir.path().join("auth.json"), r#"{"token":"REAL"}"#)
            .expect("failed to write auth.json");
        std::fs::write(codex_dir.path().join("config.toml"), "model = \"real\"")
            .expect("failed to write config.toml");

        let entries = host_codex_stage_entries(codex_dir.path())
            .expect("stat of a plain tempdir must not fail");

        assert!(
            entries
                .iter()
                .any(|(path, name)| *name == "auth.json" && path.is_file()),
            "a regular auth.json must still be included, got: {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(path, name)| *name == "config.toml" && path.is_file()),
            "a regular config.toml must still be included, got: {entries:?}"
        );
    }

    #[test]
    fn host_codex_stage_entries_skips_missing_files_silently() {
        // 回帰確認: `.codex` 配下に何も無い(config.toml が無いホストの通常
        // 運用)場合、エントリは空になる(NotFound は警告不要で黙ってスキップ)。
        let codex_dir = tempfile::tempdir().expect("failed to create codex_dir tempdir");

        let entries = host_codex_stage_entries(codex_dir.path())
            .expect("stat of a plain tempdir must not fail");

        assert!(
            entries.is_empty(),
            "an empty .codex dir must yield no stage entries, got: {entries:?}"
        );
    }

    #[test]
    fn classify_codex_stage_entries_skips_not_found_stat_errors() {
        // 回帰確認: NotFound の stat エラーは(注入した stat 関数経由でも)
        // 従来どおり黙ってスキップされ、Ok の空/部分エントリになること。
        // `host_codex_stage_entries_skips_missing_files_silently` は実ファイル
        // システム経由の同じ挙動を確認しているが、こちらは pure 関数に対して
        // NotFound を直接注入し、分岐そのものを固定する。
        let codex_dir = std::path::PathBuf::from("/nonexistent/.codex");

        let entries = classify_codex_stage_entries(&codex_dir, |_path| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such file",
            ))
        });

        assert_eq!(
            entries.expect("NotFound must not produce an Err").len(),
            0,
            "NotFound stat errors must be skipped silently, not treated as a failure"
        );
    }

    #[test]
    fn classify_codex_stage_entries_propagates_non_not_found_stat_errors() {
        // Copilot レビュー指摘(PR #56)の回帰確認: NotFound 以外の stat エラー
        // (権限エラー等)を「存在しない」として黙ってスキップすると、
        // 呼び出し元 `prepare_codex_mount` の has_auth 判定が誤って false になり、
        // auth store の wipe(P1 分岐)を誤発動させる。stat が
        // PermissionDenied で失敗した場合は Err で伝播し、エラーメッセージに
        // 対象パスが含まれることを固定する。
        let codex_dir = std::path::PathBuf::from("/no/access/.codex");

        let result = classify_codex_stage_entries(&codex_dir, |_path| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "permission denied",
            ))
        });

        let err = result
            .expect_err("a non-NotFound stat error must propagate as Err, not be silently skipped");
        let message = format!("{err:#}");
        assert!(
            message.contains("auth.json") || message.contains("config.toml"),
            "error message must name the path whose stat failed so operators can diagnose \
             the permission issue, got: {message}"
        );
        assert!(
            message.contains("permission denied"),
            "error message must retain the underlying io::Error via anyhow context chaining, \
             got: {message}"
        );
    }
}
