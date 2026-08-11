use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::config::{self, GlobalConfig};
use crate::runtime::{ContainerInfo, DockerRuntime};
use crate::ui::{banner, prompts};

/// ホストの UID/GID を返す（Dockerfile の HOST_UID / HOST_GID build-arg 用）。
/// 非 Unix では固定値 (1000, 1000) を返す。
fn host_uid_gid() -> (u32, u32) {
    #[cfg(unix)]
    {
        // SAFETY: getuid() と getgid() は前提条件のない単純なシステムコール。
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        (uid, gid)
    }
    #[cfg(not(unix))]
    {
        (1000, 1000)
    }
}

/// `docker build` に渡すビルド引数を組み立てる純関数。
///
/// docker を呼ばずに済むよう `build_image_for` から切り出している
/// （ユニットテストで `VIBEPOD_PROFILE` の組み立てを直接検証するため）。
/// `profile` が `None`（未指定）のときは Dockerfile 側の
/// `ARG VIBEPOD_PROFILE=default` と対応する `"default"` を渡す。
fn build_args_for(uid: u32, gid: u32, profile: Option<&str>) -> HashMap<String, String> {
    let mut build_args = HashMap::new();
    build_args.insert("HOST_UID".to_string(), uid.to_string());
    build_args.insert("HOST_GID".to_string(), gid.to_string());
    build_args.insert(
        "VIBEPOD_PROFILE".to_string(),
        profile.unwrap_or("default").to_string(),
    );
    build_args
}

/// 埋め込み Dockerfile とホストの UID/GID を使って vibepod イメージを
/// ビルドする共通処理。`vibepod init` と `vibepod run` の自動ビルドの
/// 両方から呼ばれ、ビルド引数の組み立てを 1 箇所に集約する。
///
/// `profile`: `Some("swift")` のように渡すと `VIBEPOD_PROFILE` ビルド引数に
/// そのまま反映される。`None` は `"default"` として渡す。呼び出し側で
/// イメージ名（`image_name`）と profile の対応を取る責務を持つ
/// （このミスマッチはビルドを壊さないが、意図しないバリアントを作る）。
pub async fn build_image_for(
    runtime: &DockerRuntime,
    image_name: &str,
    rebuild: bool,
    profile: Option<&str>,
) -> Result<()> {
    let dockerfile = include_str!("../../templates/Dockerfile");
    let (uid, gid) = host_uid_gid();

    let build_args = build_args_for(uid, gid, profile);

    runtime
        .build_image(dockerfile, image_name, build_args, rebuild)
        .await
}

/// イメージ未ビルド時に何をすべきかの純粋な判定。時刻や docker 状態に
/// 依存しないので、要否判定だけを切り出してユニットテストできる。
#[derive(Debug, PartialEq, Eq)]
pub enum AutoBuildDecision {
    /// イメージが既にあるので何もしない。
    Skip,
    /// 自動ビルドする。
    Build,
    /// イメージが無く、かつ `--no-auto-build` が指定されているので失敗させる。
    FailNoAutoBuild,
}

/// `image_exists` と `--no-auto-build` フラグから自動ビルドの要否を決める。
pub fn auto_build_decision(image_exists: bool, no_auto_build: bool) -> AutoBuildDecision {
    if image_exists {
        AutoBuildDecision::Skip
    } else if no_auto_build {
        AutoBuildDecision::FailNoAutoBuild
    } else {
        AutoBuildDecision::Build
    }
}

/// `init --rebuild` 時に profile バリアントイメージ（`VALID_PROFILES` の各
/// エントリ）も再ビルドするかどうかの純粋な判定。docker を呼ばずに済むよう
/// `execute` から切り出している（`auto_build_decision` と同じパターン）。
///
/// F7（フル再レビュー指摘）: 以前は `swift_rebuild_decision` という名前で
/// "swift" 専用のように読めたが、判定ロジック自体は元から profile 名に
/// 依存しない（`rebuild && exists` のみ）。呼び出し側（`execute`）を
/// `VALID_PROFILES` のループへ一般化したのに合わせ、profile 非依存の名前へ
/// リネームした。
///
/// 引数無し `vibepod init`（`rebuild = false`）では、対象 profile イメージが
/// 過去に作られていても再ビルドしない — 未使用の profile を勝手に
/// ビルドし始めない現行仕様（設計書 2.5）を守るための不変条件。
pub fn profile_rebuild_decision(rebuild: bool, profile_image_exists: bool) -> bool {
    rebuild && profile_image_exists
}

/// メインイメージのビルドと（`--rebuild` 時の）profile バリアント再ビルドを
/// まとめて行う。`execute` から切り出すことで、`build_then_remove_containers`
/// （Issue #71 条件1）へビルド処理を注入できるようにしている
/// （本番はこの関数を渡し、テストはダミーの処理を渡す）。
async fn build_images(runtime: &DockerRuntime, image_name: &str, rebuild: bool) -> Result<()> {
    if rebuild {
        println!(
            "\n  Rebuilding Docker image from scratch: {} (--pull --no-cache)...",
            image_name
        );
    } else {
        println!("\n  Building Docker image: {}...", image_name);
    }

    if let Err(e) = build_image_for(runtime, image_name, rebuild, None).await {
        eprintln!("\n  ✗ Build failed: {}", e);
        eprintln!("    Check your network connection and try `vibepod init` again.");
        if !rebuild {
            eprintln!(
                "    If the build succeeded but the image is stale, run `vibepod init --rebuild`."
            );
        }
        return Err(e);
    }

    // `--rebuild` のときだけ、既に docker 上にある profile バリアントイメージも
    // 同じ引数（rebuild=true, profile=<p>）で再ビルドする。`config::VALID_PROFILES`
    // の各エントリについて存在確認し、存在するものだけを対象にする（未使用の
    // profile を勝手にビルドし始めない）。`vibepod init`（rebuild なし）では
    // default イメージのみをビルドする現行仕様を変えない
    // （`profile_rebuild_decision` が守る不変条件）。
    //
    // `image_exists` の確認自体は付随処理として扱う: default イメージの
    // ビルドは既にここまでで成功しているため、確認が docker daemon 不調・
    // 権限エラー等（`Err`。イメージ未存在は `Ok(false)`）で失敗しても
    // 致命的にはしない。
    for profile in config::VALID_PROFILES {
        let profile_image_name = config::image_for_profile(image_name, profile);
        let profile_image_exists = if rebuild {
            match runtime.image_exists(&profile_image_name).await {
                Ok(exists) => exists,
                Err(e) => {
                    eprintln!(
                        "  Warning: could not check whether the {profile} variant image '{}' \
                         exists: {}. Skipping the {profile} variant rebuild check (the default \
                         image was already rebuilt successfully). Run `vibepod init --rebuild` \
                         again to retry.",
                        profile_image_name, e
                    );
                    false
                }
            }
        } else {
            false
        };

        if profile_rebuild_decision(rebuild, profile_image_exists) {
            println!(
                "\n  Rebuilding Docker image from scratch: {} (--pull --no-cache)...",
                profile_image_name
            );
            if let Err(e) = build_image_for(runtime, &profile_image_name, true, Some(profile)).await
            {
                eprintln!("\n  ✗ Build failed: {}", e);
                eprintln!(
                    "    Check your network connection and try `vibepod init --rebuild` again."
                );
                return Err(e);
            }
        }
    }

    Ok(())
}

/// 自動ビルドの同時実行を直列化するためのアドバイザリロック。
///
/// 複数セッションから同時に `vibepod run` が走ると、同名イメージのビルドが
/// 二重に起動しうる。docker build 自体は同一タグへの並行ビルドを弾かず、
/// 双方が数分かけて実質同じイメージを作り、最後にタグを取り合う（無駄 +
/// インターリーブしたログで壊れて見える）。`flock(LOCK_EX)` はブロッキング
/// なので、2 つ目のセッションはここで待ち、1 つ目がビルドを終えたあとに
/// 呼び出し側の再チェックで既存イメージを見つけてビルドをスキップする。
///
/// ロックはファイル記述子を閉じる（`_file` の drop）と解放される。
struct BuildLock {
    #[cfg(unix)]
    _file: std::fs::File,
}

impl BuildLock {
    fn acquire(config_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(config_dir).with_context(|| {
            format!(
                "Failed to create config dir for build lock: {}",
                config_dir.display()
            )
        })?;
        let path = config_dir.join("build.lock");
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .with_context(|| format!("Failed to open build lock file: {}", path.display()))?;
            // まず非ブロッキングで試す。即取れれば（競合なしの通常ケース）
            // 何も表示しない。取れない場合だけ「別セッションがビルド中で
            // 待機している」旨を出してからブロッキングで取り直す。こうしないと
            // ロック取得後に出る "Building it now..." までの数分間、待機側の
            // 画面がフリーズして見え、Ctrl+C 連打を誘発する（指摘 #3）。
            //
            // SAFETY: flock は単純なシステムコール。fd は `file` の生存期間に
            // わたって有効。LOCK_NB は即時返り、LOCK_EX はブロッキングで
            // 排他ロックを取る。
            let fd = file.as_raw_fd();
            let rc_nb = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if rc_nb != 0 {
                let err = std::io::Error::last_os_error();
                // EWOULDBLOCK: 別プロセスが保持中。待機に入ることを告知する。
                if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    println!(
                        "  Another session is building the Docker image. \
                         Waiting for it to finish (this can take a few minutes)..."
                    );
                    let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
                    if rc != 0 {
                        return Err(std::io::Error::last_os_error()).with_context(|| {
                            format!(
                                "Failed to acquire exclusive build lock on {}",
                                path.display()
                            )
                        });
                    }
                } else {
                    // EWOULDBLOCK 以外の失敗（権限・fd 異常等）はそのまま伝播。
                    return Err(err).with_context(|| {
                        format!(
                            "Failed to acquire exclusive build lock on {}",
                            path.display()
                        )
                    });
                }
            }
            Ok(Self { _file: file })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(Self {})
        }
    }
}

/// `vibepod run` から呼ばれる、イメージ存在保証。
///
/// - 既にあれば何もしない。
/// - 無く `--no-auto-build` なら、次のアクション（`vibepod init`）を示して失敗。
/// - 無く自動ビルド許可なら、ビルドロックを取って（二重ビルド防止）、待機後の
///   再チェックを挟んでからビルドする。進行中であることと失敗時の対処を
///   明示する。
///
/// `profile`: `image_name` が profile 付きイメージ（`image_for_profile` で
/// 導出したもの）のときに、対応する `VIBEPOD_PROFILE` ビルド引数を渡すため
/// の値。`image_name` と整合しない値を渡さないのは呼び出し側の責務。
pub async fn ensure_image_available(
    runtime: &DockerRuntime,
    image_name: &str,
    config_dir: &Path,
    no_auto_build: bool,
    profile: Option<&str>,
) -> Result<()> {
    let exists = runtime.image_exists(image_name).await?;
    match auto_build_decision(exists, no_auto_build) {
        AutoBuildDecision::Skip => Ok(()),
        AutoBuildDecision::FailNoAutoBuild => bail!(
            "Docker image '{}' not found and --no-auto-build was set.\n  \
             Build it once with `vibepod init` (takes a few minutes), then re-run.",
            image_name
        ),
        AutoBuildDecision::Build => {
            // 二重ビルド防止。ロック取得を待っている間に別セッションが
            // ビルドを完了しうるので、取得後にもう一度存在確認する。
            let _lock = BuildLock::acquire(config_dir)
                .context("Failed to acquire build lock for automatic image build")?;
            if runtime.image_exists(image_name).await? {
                println!(
                    "  Docker image '{}' was built by a concurrent session; continuing.",
                    image_name
                );
                return Ok(());
            }

            println!();
            println!(
                "  Docker image '{}' not found. Building it now — this can take a few minutes.",
                image_name
            );
            println!(
                "  (Run `vibepod init` beforehand to avoid this wait, or pass --no-auto-build to fail fast.)"
            );
            println!();

            build_image_for(runtime, image_name, false, profile)
                .await
                .with_context(|| {
                    format!(
                        "Automatic build of Docker image '{}' failed. Check your network \
                         connection and free disk space, then run `vibepod init` to see the \
                         full build output and retry.",
                        image_name
                    )
                })?;

            println!("  Image '{}' built. Continuing with the run.", image_name);
            Ok(())
        }
    }
}

/// 非対話環境でも `vibepod init` を落とさないための agent 選択判定。
///
/// docker を呼ばず TTY 判定だけに依存する分岐なので、`execute` から切り出して
/// ユニットテストできるようにしている（`auto_build_decision` と同じパターン）。
///
/// `is_interactive` が true（対話端末）のときだけ `prompts::select_agent()` で
/// `dialoguer::Select` による選択プロンプトを出す。false（CI・パイプ・
/// スクリプト経由など、Issue #67 で報告された `vibepod init` の非TTY実行）の
/// ときは `prompts::select_agent()` を呼ばず（呼ぶと `IO error: not a
/// terminal` で落ちる）、`GlobalConfig::default().default_agent`
/// （`src/config/global.rs`）を既定値として使う。リテラルで二重管理せず
/// 同じ値を参照することで、将来デフォルトが変わってもここが追従する。
/// `prompts::select_agent()` の他の選択肢（Gemini CLI / OpenAI Codex）も
/// 現状すべて内部で "claude" にフォールバックする実装（`src/ui/prompts.rs`）
/// なので、対話・非対話のどちらでも実質的に選ばれる agent は変わらない。
/// 暗黙にデフォルトへフォールバックしたことを運用者が追えるよう、
/// 非対話時は stderr に警告を出す。
///
/// 注意: config.toml 手編集・対話再実行は次のアクションとして案内しない。
/// `default_agent` は毎回上書き保存される dead field で手編集はすぐ失われ、
/// 対話再実行も上記フォールバックにより結果が変わらないため。
fn resolve_agent(is_interactive: bool) -> Result<String> {
    if is_interactive {
        prompts::select_agent()
    } else {
        let default_agent = GlobalConfig::default().default_agent;
        eprintln!(
            "  Warning: No interactive terminal detected; using default agent '{}' \
             (non-interactive mode). It's currently the only supported agent, so the \
             other choices in the interactive prompt (Gemini CLI, OpenAI Codex) would \
             resolve to it too.",
            default_agent
        );
        Ok(default_agent)
    }
}

/// 削除して安全と判定できる docker の `{{.State}}` 値。
///
/// これ以外（running / restarting / paused、および将来 docker が追加する
/// 未知の state）は保護対象として扱う。値は docker が実際に返す小文字表記
/// （`docker ps --format '{{.State}}'`）に合わせている。
const REMOVABLE_STATES: &[&str] = &["exited", "created", "dead"];

/// docker のコンテナ state が「保護対象」（無確認で触れてはならない）か
/// どうかを判定する純関数。未知の state は安全側（保護）に倒す。
///
/// この判定結果は `container_removal_decision` の分岐そのものには使わない
/// （分岐はコンテナの有無のみで決まる）。確認・abort メッセージに含める
/// 「うち N 件が保護対象」の件数算出にのみ使う。
fn is_protected_state(state: &str) -> bool {
    !REMOVABLE_STATES.contains(&state.to_lowercase().as_str())
}

/// 非対話環境でも `vibepod init` を落とさない・かつ他プロジェクトの
/// セッションを壊さないための、既存コンテナ削除の判定。
///
/// docker を呼ばず TTY 判定とコンテナ件数だけに依存する分岐なので、
/// `execute` から切り出してユニットテストできるようにしている
/// （`auto_build_decision` / `resolve_agent` と同じパターン）。
///
/// 稼働中コンテナ数では**分岐しない**。vibepod は非 disposable コンテナを
/// 停止保持して再利用する設計であり、停止中（`exited`）のコンテナにも
/// resume 可能なセッション状態が残る。「稼働中が 0 件だから消してよい」
/// という前提は誤りであり、無確認削除の経路はコンテナが 1 件も存在しない
/// 場合（削除対象そのものが無い no-op）にのみ限定する。
///
/// - コンテナが 0 件なら確認なしで続行する（実際には削除対象が無い no-op）。
/// - コンテナが 1 件以上かつ対話端末（`is_interactive` = true）なら
///   `dialoguer::Confirm` の確認プロンプトへ進む。
/// - コンテナが 1 件以上かつ非対話（CI・パイプ経由など）は、確認を
///   取れないため削除せずエラーで中断する。`list_vibepod_containers()` は
///   プロジェクト横断で全 vibepod コンテナを返すため、ここで確認なしに
///   削除すると他プロジェクトのコンテナ（稼働中・停止中を問わず、停止中
///   なら resume 可能な状態を含む）まで巻き込んで壊しうる。
#[derive(Debug, PartialEq, Eq)]
enum ContainerRemovalDecision {
    /// 確認なしで続行する（コンテナが無いので実質 no-op）。
    Remove,
    /// `dialoguer::Confirm` で確認を取ってから決める。
    Confirm,
    /// 確認が取れないため削除せずエラー終了する。
    Abort,
}

fn container_removal_decision(
    is_interactive: bool,
    container_count: usize,
) -> ContainerRemovalDecision {
    if container_count == 0 {
        ContainerRemovalDecision::Remove
    } else if is_interactive {
        ContainerRemovalDecision::Confirm
    } else {
        ContainerRemovalDecision::Abort
    }
}

/// 非対話 abort 時のエラーメッセージを組み立てる純関数。
///
/// `build_completed` が true（削除直前チェックでの abort）のときだけ、
/// イメージビルドが既に完了し `latest` タグも更新済みである旨を含める
/// （ビルド前チェックでの abort はビルド自体が走っていないため不要）。
fn non_interactive_abort_message(total: usize, protected: usize, build_completed: bool) -> String {
    let build_note = if build_completed {
        "\n  Note: the Docker image was already rebuilt and its `latest` tag updated \
         before this check ran — only the container removal step was aborted."
    } else {
        ""
    };
    format!(
        "{} VibePod container(s) found across projects ({} of them protected: running, \
         restarting, paused, or unknown state), but this session is non-interactive (stderr is \
         not a terminal) so a removal confirmation cannot be obtained. Aborting without \
         touching them.{}\n  \
         `vibepod init` removes ALL VibePod containers across every project once it proceeds — \
         including stopped ones holding other sessions' resumable state — so a confirmation is \
         required whenever at least one container exists, running or not.\n  \
         Run `vibepod ps` first to see what currently exists.\n  \
         Re-run `vibepod init` from an interactive terminal to get a confirmation prompt, or \
         remove the containers yourself first (e.g. `vibepod rm --all` from an interactive \
         terminal) so none remain.",
        total, protected, build_note
    )
}

/// コンテナ削除まわりの安全判定（`container_removal_decision` /
/// `is_protected_state`）を実際に守っているのが、列挙・削除の呼び出し側
/// （`execute`）であることをテストで固定するための最小 trait。
///
/// `DockerRuntime` 全体を trait 化すると呼び出し元 5 ファイル
/// （`run/prepare.rs` / `stop.rs` / `ps.rs` / `rm.rs` / `logs.rs`）の
/// シグネチャに波及するため、ここでは検証したい 2 メソッドだけを
/// 切り出す（全面 trait 化は Issue #73 として別管理）。
///
/// Rust 1.75 で安定化済みの `async fn in trait` を使い `async-trait` は
/// 追加しない。この形は object safety を持たないため、呼び出し側は
/// `dyn ContainerRegistry` ではなくジェネリクス（`R: ContainerRegistry`）
/// で受ける。
///
/// trait を pub にせず crate 内部（`init` モジュール限定）に留めることで
/// `clippy::async_fn_in_trait` の警告を避けている。
trait ContainerRegistry {
    async fn list_vibepod_containers(&self) -> Result<Vec<ContainerInfo>>;
    async fn remove_container(&self, name: &str) -> Result<()>;
}

impl ContainerRegistry for DockerRuntime {
    async fn list_vibepod_containers(&self) -> Result<Vec<ContainerInfo>> {
        // 右辺はメソッド呼び出し構文の解決規則により inherent メソッド
        // （`DockerRuntime::list_vibepod_containers`）が優先されるため、
        // trait 経由の無限再帰にはならない。
        self.list_vibepod_containers().await
    }

    async fn remove_container(&self, name: &str) -> Result<()> {
        self.remove_container(name).await
    }
}

/// ビルド前チェック（Issue #69）: 非対話かつコンテナが存在する場合に
/// ビルドへ入る前に fail-fast させる。削除は行わない（判定のみ）。
///
/// `execute` から切り出すことで、fake `ContainerRegistry` を渡して
/// 「非対話 + コンテナありでエラーになり、かつ削除が一切呼ばれない」
/// ことをテストできるようにしている。
async fn fail_fast_if_removal_would_abort<R: ContainerRegistry>(
    registry: &R,
    is_interactive: bool,
) -> Result<()> {
    let containers = registry.list_vibepod_containers().await?;
    if let ContainerRemovalDecision::Abort =
        container_removal_decision(is_interactive, containers.len())
    {
        let protected = containers
            .iter()
            .filter(|c| is_protected_state(&c.state))
            .count();
        bail!(non_interactive_abort_message(
            containers.len(),
            protected,
            false
        ));
    }
    Ok(())
}

/// 削除直前の再列挙・判定・実削除（TOCTOU 対策）。
///
/// `confirm`: 対話端末での確認結果を注入できるようにするためのクロージャ。
/// 実 TTY を要求する `dialoguer::Confirm`（`prompts::confirm_remove_all_containers`）
/// はテスト対象外とし、本番コードでは `execute()` からそのまま渡す。
///
/// 戻り値: `Ok(true)` は削除処理を終えて config 保存へ進んでよいこと
/// （実際に削除した、またはコンテナが 0 件で no-op だった）を示す。
/// `Ok(false)` はユーザーが確認を拒否し削除しなかったことを示し、
/// `execute()` はこの場合 config を更新せず早期リターンする。
async fn remove_existing_containers<R, F>(
    registry: &R,
    is_interactive: bool,
    confirm: F,
) -> Result<bool>
where
    R: ContainerRegistry,
    F: FnOnce(usize, usize) -> Result<bool>,
{
    let containers = registry.list_vibepod_containers().await?;
    if containers.is_empty() {
        return Ok(true);
    }

    let protected_count = containers
        .iter()
        .filter(|c| is_protected_state(&c.state))
        .count();

    let should_remove = match container_removal_decision(is_interactive, containers.len()) {
        ContainerRemovalDecision::Remove => true,
        ContainerRemovalDecision::Confirm => confirm(containers.len(), protected_count)?,
        ContainerRemovalDecision::Abort => {
            bail!(non_interactive_abort_message(
                containers.len(),
                protected_count,
                true
            ));
        }
    };

    if should_remove {
        println!("  Removing {} existing container(s)...", containers.len());
        for container in &containers {
            registry.remove_container(&container.name).await?;
        }
        println!("  Removed {} container(s).", containers.len());
        Ok(true)
    } else {
        eprintln!(
            "  Skipping config update: existing containers were not removed. \
             Re-run `vibepod init` and remove containers to apply the new image."
        );
        Ok(false)
    }
}

/// コンテナ安全処理のオーケストレーション（Issue #71 条件1）。
///
/// 本番 `execute()` の接続順序（ビルド前チェック → イメージビルド →
/// 削除直前チェック・削除）を 1 つの関数へ切り出し、本番とテストの両方が
/// この関数を経由するようにする。これにより「テストはヘルパーを直接順番に
/// 呼ぶだけで、`execute()` 側の接続が壊れても検出できない」という穴を防ぐ
/// （テストは `build` にダミー処理を注入してこの関数を直接呼ぶ）。
///
/// `build`: ビルド処理を注入するためのクロージャ。本番は `build_images` を
/// 渡し、テストは「何もしない」または「コンテナが増える状況をシミュレート
/// する」処理を渡す。`FnOnce() -> Fut` という形は stable Rust に安定化済みの
/// async closure を使わず、通常のクロージャに async block を返させることで
/// 実現している。
///
/// 戻り値は `remove_existing_containers` と同じ意味を持つ
/// （`Ok(true)`: 削除完了または no-op、`Ok(false)`: 確認拒否で未削除）。
async fn build_then_remove_containers<R, B, Fut, F>(
    registry: &R,
    is_interactive: bool,
    build: B,
    confirm: F,
) -> Result<bool>
where
    R: ContainerRegistry,
    B: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
    F: FnOnce(usize, usize) -> Result<bool>,
{
    fail_fast_if_removal_would_abort(registry, is_interactive).await?;
    build().await?;
    remove_existing_containers(registry, is_interactive, confirm).await
}

/// `rebuild`: pass `--pull --no-cache` to `docker build` so the image is
/// reconstructed from scratch. Needed to pick up a newer Claude Code, since
/// the `install.sh` layer is otherwise served from cache forever.
pub async fn execute(rebuild: bool) -> Result<()> {
    banner::print_banner();

    // 1. Check Docker
    let runtime = DockerRuntime::new()
        .await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;
    runtime.ping().await?;

    // 2. Select agent
    //
    // dialoguer の `Select`/`Confirm` は `Term::stderr()` を使って対話する
    // （dialoguer-0.11.0/src/prompts/select.rs 等）ため、TTY 判定は stdin
    // ではなく stderr で行う。stdin だけで判定すると、`vibepod init 2>&1 |
    // tee log` のように stdin は TTY でも stderr がパイプされているケースを
    // 対話と誤判定し、dialoguer が "IO error: not a terminal" でクラッシュ
    // する。3-5. のコンテナ削除確認でも同じ理由でこの値を使い回す。
    let is_interactive = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let agent = resolve_agent(is_interactive)?;

    // 3-5. ビルド前チェック → イメージビルド → 削除直前チェック・削除
    //
    // 非対話 CI で `--rebuild` を実行すると、`--pull --no-cache` を伴う
    // 数分〜十数分のビルドを完走してから、後段のコンテナ削除確認で必ず
    // 失敗していた（「ビルドは成功したのに init 全体は失敗し、しかも latest
    // タグだけ更新済み」という部分成功状態を残す）。abort になることが
    // 事前に分かっている非対話ケースは、ビルドに入る前に fail fast させる
    // （Issue #69）。
    //
    // ビルドには数分〜十数分かかることがあり、その間に別プロセスが
    // `vibepod run` を開始してコンテナが増えている可能性があるため、ビルド後
    // にもう一度列挙・判定し直す（TOCTOU 対策）。コンテナが 1 件以上ある場合、
    // 対話端末なら確認プロンプトを表示し、非対話なら確認が取れないため削除
    // せずエラー終了する（稼働中・停止中を問わない。停止中コンテナにも
    // resume 可能な状態が残るため）。
    //
    // この 3 段（ビルド前チェック → ビルド → 削除直前チェック・削除）の接続
    // 順序は `build_then_remove_containers`（Issue #71 条件1）に切り出して
    // おり、テストも同じ関数を経由して固定している。ここで直接ヘルパーを
    // 順番に呼ぶと、将来この接続が壊れてもテストで検出できない。
    let image_name = format!("vibepod-{}:latest", agent);
    let should_continue = build_then_remove_containers(
        &runtime,
        is_interactive,
        || build_images(&runtime, &image_name, rebuild),
        |total, protected| {
            // インタラクティブ + コンテナあり: 確認プロンプト
            prompts::confirm_remove_all_containers(total, protected)
        },
    )
    .await?;
    if !should_continue {
        // ユーザーがコンテナ削除を拒否 → config を更新しない（旧コンテナが旧イメージのまま残る）
        return Ok(());
    }

    // 6. Save config（コンテナ削除後に保存することで、削除キャンセル時に旧イメージが残ったまま
    //    config が更新される問題を回避する）
    let config_dir = config::default_config_dir()?;
    let config = GlobalConfig {
        default_agent: agent,
        image: image_name,
    };
    config::save_global_config(&config, &config_dir)?;

    println!("\n  Done! Run `vibepod run` in any git repo to start.\n");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // テスト計画 第5節 項目4: `build_image_for` 相当のビルド引数組み立てに
    // `VIBEPOD_PROFILE` が含まれることの検証。docker を実際に呼ばずに済むよう
    // `build_image_for` から切り出した純関数 `build_args_for` を直接検証する。

    #[test]
    fn build_args_include_host_uid_and_gid() {
        let args = build_args_for(1000, 1000, None);
        assert_eq!(args.get("HOST_UID"), Some(&"1000".to_string()));
        assert_eq!(args.get("HOST_GID"), Some(&"1000".to_string()));
    }

    #[test]
    fn build_args_default_profile_when_none() {
        let args = build_args_for(1000, 1000, None);
        assert_eq!(args.get("VIBEPOD_PROFILE"), Some(&"default".to_string()));
    }

    #[test]
    fn build_args_swift_profile_when_specified() {
        let args = build_args_for(1000, 1000, Some("swift"));
        assert_eq!(args.get("VIBEPOD_PROFILE"), Some(&"swift".to_string()));
    }

    // doc comment を参照。`is_interactive = false` 分岐が dialoguer を経由しない
    // ことを固定するテスト。
    #[test]
    fn resolve_agent_non_interactive_uses_default_agent() {
        // GlobalConfig::default().default_agent（src/config/global.rs）を
        // リテラルで複製せず、プロンプトを呼ばずに同じ値が返ることを検証する。
        let agent = resolve_agent(false).unwrap();
        assert_eq!(agent, GlobalConfig::default().default_agent);
    }

    // Critical 1 回帰テスト: コンテナが 0 件なら対話・非対話を問わず
    // 確認なしで続行する（削除対象そのものが無い no-op）。
    #[test]
    fn container_removal_decision_no_containers_removes_regardless_of_interactivity() {
        assert_eq!(
            container_removal_decision(false, 0),
            ContainerRemovalDecision::Remove
        );
        assert_eq!(
            container_removal_decision(true, 0),
            ContainerRemovalDecision::Remove
        );
    }

    // Critical 1 回帰テスト: コンテナが 1 件でもあれば非対話は確認なしに
    // 削除できないためエラー終了する。停止中コンテナだけでも resume 可能な
    // セッション状態を壊しうるため、「稼働中が 0 件だから削除してよい」と
    // いう旧仕様の前提そのものをここで否定する。
    #[test]
    fn container_removal_decision_with_containers_non_interactive_aborts() {
        assert_eq!(
            container_removal_decision(false, 1),
            ContainerRemovalDecision::Abort
        );
        assert_eq!(
            container_removal_decision(false, 3),
            ContainerRemovalDecision::Abort
        );
    }

    // コンテナが 1 件以上かつ対話端末なら、稼働中・停止中を問わず確認
    // プロンプトへ進む（無確認削除の経路は存在しない）。
    #[test]
    fn container_removal_decision_with_containers_interactive_confirms() {
        assert_eq!(
            container_removal_decision(true, 1),
            ContainerRemovalDecision::Confirm
        );
        assert_eq!(
            container_removal_decision(true, 3),
            ContainerRemovalDecision::Confirm
        );
    }

    // Critical 2 回帰テスト: `is_protected_state` の表形式テスト。
    // running/restarting/paused は保護対象、exited/created/dead は削除して
    // 安全、未知の state（将来 docker が追加しうる値を含む）は安全側の
    // 保護対象に倒れることを固定する。
    #[test]
    fn is_protected_state_table() {
        let cases: &[(&str, bool)] = &[
            ("running", true),
            ("restarting", true),
            ("paused", true),
            ("exited", false),
            ("created", false),
            ("dead", false),
            ("weird-new-state", true),
        ];
        for (state, expected_protected) in cases {
            assert_eq!(
                is_protected_state(state),
                *expected_protected,
                "state = {:?}",
                state
            );
        }
    }

    // --- Issue #71: ContainerRegistry を介した統合テスト ---
    //
    // 上の純関数テストは `container_removal_decision` / `is_protected_state`
    // 自体の仕様を固定している。ここからは、実際に安全性を担っている
    // 呼び出し側（`fail_fast_if_removal_would_abort` /
    // `remove_existing_containers`）が、その判定を無視して削除を呼んで
    // しまう回帰（例: 非対話環境から他プロジェクトの停止中コンテナを
    // 無確認削除してしまう）を検出する。

    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// テスト用 fake `ContainerRegistry`。
    ///
    /// `list_vibepod_containers` は呼び出しごとに `responses` から 1 件ずつ
    /// 消費して返す（ビルド前後で異なる一覧を返す必要があるテスト用）。
    /// `responses` が枯渇した状態で呼ばれた場合は `expect` で即座に panic
    /// させる（Issue #71 条件2）: 実装の `list_vibepod_containers` は毎回
    /// docker を実行するため「枯渇して空になる」挙動を持たず、fake が
    /// 想定外の追加呼び出しを空一覧（無確認続行を意味する安全上重要な値）に
    /// 変換してしまうと、fake の設定ミスや接続回数の回帰を握り潰してしまう。
    ///
    /// `remove_container` は呼ばれた名前を `removed` に記録する。
    /// `fail_remove_on`（1-based）が指定されていれば、その回数目の呼び出しで
    /// 記録せずエラーを返す（削除失敗の伝播をテストするため）。
    struct FakeRegistry {
        responses: Mutex<VecDeque<Result<Vec<ContainerInfo>>>>,
        removed: Arc<Mutex<Vec<String>>>,
        calls: AtomicUsize,
        fail_remove_on: Option<usize>,
    }

    impl FakeRegistry {
        fn new(responses: Vec<Vec<ContainerInfo>>) -> Self {
            Self::new_with_list_results(responses.into_iter().map(Ok).collect())
        }

        /// 列挙自体を失敗させたいテスト用に、`Result` を直接注入するコンストラクタ。
        fn new_with_list_results(responses: Vec<Result<Vec<ContainerInfo>>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                removed: Arc::new(Mutex::new(Vec::new())),
                calls: AtomicUsize::new(0),
                fail_remove_on: None,
            }
        }

        /// `nth_call` 回目（1-based）の `remove_container` 呼び出しを失敗させる。
        fn with_failing_remove_on(mut self, nth_call: usize) -> Self {
            self.fail_remove_on = Some(nth_call);
            self
        }

        fn removed_names(&self) -> Vec<String> {
            self.removed.lock().unwrap().clone()
        }

        /// `build` クロージャと `remove_container` の両方から同じログへ
        /// 書き込ませ、実行順序（ビルドが削除より先に完了しているか）を
        /// 直接検証するためのハンドル（Issue #71 条件1）。
        fn removed_handle(&self) -> Arc<Mutex<Vec<String>>> {
            self.removed.clone()
        }

        /// `list_vibepod_containers` が呼ばれた回数。
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// 未消費の `responses` の件数（テストが用意した件数と実際の呼び出し
        /// 回数の食い違いを検証できるようにする）。
        fn remaining_responses(&self) -> usize {
            self.responses.lock().unwrap().len()
        }
    }

    impl ContainerRegistry for FakeRegistry {
        async fn list_vibepod_containers(&self) -> Result<Vec<ContainerInfo>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().unwrap();
            responses
                .pop_front()
                .expect("unexpected list_vibepod_containers call")
        }

        async fn remove_container(&self, name: &str) -> Result<()> {
            let mut removed = self.removed.lock().unwrap();
            let call_index = removed.len() + 1;
            if self.fail_remove_on == Some(call_index) {
                bail!("fake remove_container failure for {}", name);
            }
            removed.push(name.to_string());
            Ok(())
        }
    }

    fn container(name: &str, state: &str) -> ContainerInfo {
        ContainerInfo {
            name: name.to_string(),
            state: state.to_string(),
            status: state.to_string(),
        }
    }

    /// 呼ばれたら test を fail させる confirm クロージャ。
    /// 「確認プロンプトへ進むはずがない」分岐（Remove / Abort）で
    /// 誤って confirm が呼ばれていないことを検証するために使う。
    fn confirm_must_not_be_called(_total: usize, _protected: usize) -> Result<bool> {
        panic!("confirm should not be called on this path");
    }

    // 項目1: 非対話 + コンテナ1件以上（停止中のみ）→ エラーになり、
    // remove_container が一度も呼ばれない。
    #[tokio::test]
    async fn non_interactive_with_stopped_container_aborts_without_removing() {
        let registry = FakeRegistry::new(vec![vec![container("vibepod-other-a", "exited")]]);
        let result = fail_fast_if_removal_would_abort(&registry, false).await;
        assert!(result.is_err());
        assert!(registry.removed_names().is_empty());

        // 削除直前チェック側（TOCTOU 対策の再列挙）でも同じ入力で同じ結果になる
        // ことを確認する。Abort 分岐では confirm を呼ばない。
        let registry = FakeRegistry::new(vec![vec![container("vibepod-other-a", "exited")]]);
        let result = remove_existing_containers(&registry, false, confirm_must_not_be_called).await;
        assert!(result.is_err());
        assert!(registry.removed_names().is_empty());
    }

    // 項目2: 非対話 + ビルド前0件 + ビルド後1件（注入したビルド処理内で
    // コンテナが増える状況をシミュレート）→ 注入処理は実行されるが、
    // ビルド後の判定（`remove_existing_containers` の再列挙）で中断し、
    // remove_container が呼ばれない。ビルド前チェックの結果を使い回さず、
    // 再列挙の結果が判定に使われていることを固定する（TOCTOU 対策の回帰検出）。
    //
    // `execute()` 本番と同じ `build_then_remove_containers` を経由することで、
    // ヘルパーを直接順番に呼ぶだけでは検出できない「本番の接続順序が壊れる」
    // 回帰（削除直前チェックの除去・順序入れ替え・別の削除ループの追加）も
    // 合わせて検出する（Issue #71 条件1）。列挙がちょうど 2 回（ビルド前・
    // ビルド後）だけ行われたことも固定する（Issue #71 条件2）。
    #[tokio::test]
    async fn non_interactive_container_appearing_after_build_aborts_on_recheck() {
        let registry = FakeRegistry::new(vec![
            vec![],                                       // ビルド前: 0 件
            vec![container("vibepod-other-b", "exited")], // ビルド後: 1 件
        ]);
        let build_called = Arc::new(Mutex::new(false));
        let build_called_clone = build_called.clone();

        let result = build_then_remove_containers(
            &registry,
            false,
            move || {
                let build_called_clone = build_called_clone.clone();
                async move {
                    *build_called_clone.lock().unwrap() = true;
                    Ok(())
                }
            },
            confirm_must_not_be_called,
        )
        .await;

        assert!(
            result.is_err(),
            "ビルド後の再列挙で 1 件見つかり中断するはず"
        );
        assert!(
            *build_called.lock().unwrap(),
            "ビルド前チェックは通過するので注入処理は実行されているはず"
        );
        assert!(registry.removed_names().is_empty());
        assert_eq!(
            registry.call_count(),
            2,
            "列挙はビルド前・ビルド後の2回だけ行われるはず"
        );
        assert_eq!(registry.remaining_responses(), 0);
    }

    // 項目3: コンテナ0件 → remove_container が呼ばれない。
    #[tokio::test]
    async fn no_containers_never_calls_remove() {
        let registry = FakeRegistry::new(vec![vec![]]);
        let result = remove_existing_containers(&registry, true, confirm_must_not_be_called).await;
        assert!(result.unwrap());
        assert!(registry.removed_names().is_empty());
    }

    // 項目4: 対話 + コンテナ1件以上 + 確認拒否 → remove_container が
    // 呼ばれない。
    #[tokio::test]
    async fn interactive_confirm_declined_does_not_remove() {
        let registry = FakeRegistry::new(vec![vec![
            container("vibepod-other-c", "exited"),
            container("vibepod-other-d", "running"),
        ]]);
        let result = remove_existing_containers(&registry, true, |_total, _protected| Ok(false))
            .await
            .unwrap();
        assert!(!result);
        assert!(registry.removed_names().is_empty());
    }

    // 項目5: 対話 + コンテナ1件以上 + 確認承認 → 列挙されたすべての
    // コンテナに対して remove_container が呼ばれる。
    #[tokio::test]
    async fn interactive_confirm_approved_removes_all_listed_containers() {
        let registry = FakeRegistry::new(vec![vec![
            container("vibepod-other-e", "exited"),
            container("vibepod-other-f", "running"),
        ]]);
        let result = remove_existing_containers(&registry, true, |_total, _protected| Ok(true))
            .await
            .unwrap();
        assert!(result);
        let mut removed = registry.removed_names();
        removed.sort();
        assert_eq!(removed, vec!["vibepod-other-e", "vibepod-other-f"]);
    }

    // --- Issue #71 条件1: 本番と同じ接続順序（ビルド前チェック → 注入した
    // ビルド処理 → 削除直前チェック・削除）を `build_then_remove_containers`
    // 経由で固定する。`execute()` はこの関数を呼ぶだけなので、ここで固定した
    // 接続順序がそのまま本番の接続順序になる。

    // 条件1 項目1: ビルド前チェックで abort する場合、注入したビルド処理は
    // 実行されない（ビルドまで到達しない）。
    #[tokio::test]
    async fn build_then_remove_containers_aborts_before_build_when_precheck_fails() {
        let registry = FakeRegistry::new(vec![vec![container("vibepod-other-k", "exited")]]);
        let build_called = Arc::new(Mutex::new(false));
        let build_called_clone = build_called.clone();

        let result = build_then_remove_containers(
            &registry,
            false,
            move || {
                let build_called_clone = build_called_clone.clone();
                async move {
                    *build_called_clone.lock().unwrap() = true;
                    Ok(())
                }
            },
            confirm_must_not_be_called,
        )
        .await;

        assert!(result.is_err());
        assert!(
            !*build_called.lock().unwrap(),
            "ビルド前チェックで abort するので注入処理は実行されないはず"
        );
        assert!(registry.removed_names().is_empty());
        assert_eq!(
            registry.call_count(),
            1,
            "ビルド前チェックの1回だけ列挙されるはず"
        );
    }

    // 条件1 項目3: 正常系（対話 + 確認承認）で、注入したビルド処理が実行され、
    // その後に削除が実行される。`removed_handle()` を使い、ビルド処理と削除
    // 処理の両方に同じログへ書き込ませることで、順序そのものを検証する
    // （単に両方が呼ばれたことだけを assert すると、順序が入れ替わる回帰を
    // 見逃す）。
    #[tokio::test]
    async fn build_then_remove_containers_runs_build_before_removal_on_happy_path() {
        let registry = FakeRegistry::new(vec![
            vec![],                                       // ビルド前: 0 件
            vec![container("vibepod-other-g", "exited")], // ビルド後: 1 件
        ]);
        let log = registry.removed_handle();
        let build_log = log.clone();

        let result = build_then_remove_containers(
            &registry,
            true,
            move || {
                let build_log = build_log.clone();
                async move {
                    build_log.lock().unwrap().push("build".to_string());
                    Ok(())
                }
            },
            |_total, _protected| Ok(true),
        )
        .await;

        assert!(result.unwrap());
        assert_eq!(
            registry.removed_names(),
            vec!["build".to_string(), "vibepod-other-g".to_string()],
            "注入したビルド処理が削除より先に完了しているはず"
        );
        assert_eq!(registry.call_count(), 2);
    }

    // --- Issue #71 Suggestion: fake がエラー経路を表現できるようにし、
    // 削除側の失敗伝播を固定する。

    // 列挙が失敗した場合、削除が0回で呼び出し元へエラーが伝播する。
    #[tokio::test]
    async fn list_failure_propagates_without_removing() {
        let registry =
            FakeRegistry::new_with_list_results(vec![Err(anyhow::anyhow!("docker ps failed"))]);
        let result = remove_existing_containers(&registry, true, confirm_must_not_be_called).await;
        assert!(result.is_err());
        assert!(registry.removed_names().is_empty());
    }

    // 削除が途中で失敗した場合、そのエラーが呼び出し元へ伝播し、後続の
    // 削除（3件目以降）へ進まない。
    #[tokio::test]
    async fn remove_failure_stops_before_later_removals_and_propagates() {
        let registry = FakeRegistry::new(vec![vec![
            container("vibepod-other-h", "exited"),
            container("vibepod-other-i", "exited"),
            container("vibepod-other-j", "exited"),
        ]])
        .with_failing_remove_on(2);

        let result =
            remove_existing_containers(&registry, true, |_total, _protected| Ok(true)).await;

        assert!(result.is_err());
        assert_eq!(
            registry.removed_names().len(),
            1,
            "2件目の削除で失敗するので、1件目のみ削除済みのはず"
        );
    }
}
