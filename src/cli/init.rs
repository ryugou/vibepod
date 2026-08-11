use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::config::{self, GlobalConfig};
use crate::runtime::DockerRuntime;
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
    // する。3. と 5. のコンテナ削除確認でも同じ理由でこの値を使い回す。
    let is_interactive = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let agent = resolve_agent(is_interactive)?;

    // 3. Pre-build container check（Issue #69）
    //
    // 非対話 CI で `--rebuild` を実行すると、`--pull --no-cache` を伴う
    // 数分〜十数分のビルドを完走してから、後段のコンテナ削除確認で必ず
    // 失敗していた（「ビルドは成功したのに init 全体は失敗し、しかも latest
    // タグだけ更新済み」という部分成功状態を残す）。abort になることが
    // 事前に分かっている非対話ケースは、ビルドに入る前に fail fast させる。
    //
    // ここでは Abort 分岐だけを見る。Remove（コンテナ 0 件）や Confirm
    // （対話 + コンテナあり）は、実際の削除判断をビルド後の再チェック
    // （TOCTOU 対策、下記 5.）に委ねるため、ここでは何もしない。
    {
        let containers = runtime.list_vibepod_containers().await?;
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
    }

    // 4. Build image
    let image_name = format!("vibepod-{}:latest", agent);

    if rebuild {
        println!(
            "\n  Rebuilding Docker image from scratch: {} (--pull --no-cache)...",
            image_name
        );
    } else {
        println!("\n  Building Docker image: {}...", image_name);
    }

    match build_image_for(&runtime, &image_name, rebuild, None).await {
        Ok(_) => {}
        Err(e) => {
            eprintln!("\n  ✗ Build failed: {}", e);
            eprintln!("    Check your network connection and try `vibepod init` again.");
            if !rebuild {
                eprintln!("    If the build succeeded but the image is stale, run `vibepod init --rebuild`.");
            }
            return Err(e);
        }
    }

    // 4b. `--rebuild` のときだけ、既に docker 上にある profile バリアント
    //     イメージも同じ引数（rebuild=true, profile=<p>）で再ビルドする。
    //     `config::VALID_PROFILES` の各エントリについて存在確認し、存在する
    //     ものだけを対象にする（未使用の profile を勝手にビルドし始めない）。
    //     `vibepod init`（rebuild なし）では default イメージのみをビルドする
    //     現行仕様を変えない（`profile_rebuild_decision` が守る不変条件）。
    //
    //     F7（フル再レビュー指摘）: 以前は "swift" 一つだけを決め打ちしており、
    //     `VALID_PROFILES` に新しい profile を追加してもここが追随せず、
    //     `vibepod init --rebuild` がその profile のイメージを再ビルドし忘れる
    //     （古いイメージのまま使われ続ける）バグを生みかねなかった。
    //     `VALID_PROFILES` をループする形に一般化し、profile 追加時の
    //     rebuild 漏れを構造的に防ぐ。
    //
    //     `image_exists` の確認自体は付随処理として扱う: default イメージの
    //     ビルドは既にここまでで成功しているため、確認が docker daemon 不調・
    //     権限エラー等（`Err`。イメージ未存在は `Ok(false)`）で失敗しても
    //     致命的にはしない。ここで異常終了すると、後段のコンテナ削除・
    //     `save_global_config` に到達できず、真因が伝わらないまま
    //     `~/.config/vibepod/config.toml` が更新されず後続の `vibepod run` が
    //     「Config not found」で失敗する — ユーザーからは無関係に見える。
    for profile in config::VALID_PROFILES {
        let profile_image_name = config::image_for_profile(&image_name, profile);
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
            if let Err(e) =
                build_image_for(&runtime, &profile_image_name, true, Some(profile)).await
            {
                eprintln!("\n  ✗ Build failed: {}", e);
                eprintln!(
                    "    Check your network connection and try `vibepod init --rebuild` again."
                );
                return Err(e);
            }
        }
    }

    // 5. イメージ再ビルド後に既存のコンテナを全削除する（config 保存前に行う）
    //
    //    3. のビルド前チェックとは別に、ここでもう一度列挙・判定し直す
    //    （TOCTOU 対策）。ビルドには数分〜十数分かかることがあり、その間に別
    //    プロセスが `vibepod run` を開始してコンテナが増えている可能性が
    //    あるため、ビルド前の判定結果をそのまま使い回さない。
    //
    //    コンテナが 1 件以上ある場合、対話端末なら確認プロンプトを表示し、
    //    非対話なら確認が取れないため削除せずエラー終了する（稼働中・停止中
    //    を問わない。停止中コンテナにも resume 可能な状態が残るため）。
    let containers = runtime.list_vibepod_containers().await?;
    if !containers.is_empty() {
        let protected_count = containers
            .iter()
            .filter(|c| is_protected_state(&c.state))
            .count();

        let should_remove = match container_removal_decision(is_interactive, containers.len()) {
            ContainerRemovalDecision::Remove => true,
            ContainerRemovalDecision::Confirm => {
                // インタラクティブ + コンテナあり: 確認プロンプト
                prompts::confirm_remove_all_containers(containers.len(), protected_count)?
            }
            ContainerRemovalDecision::Abort => {
                // 非インタラクティブ + コンテナあり: `list_vibepod_containers()` は
                // プロジェクト横断で全 vibepod コンテナを返すため、ここで確認なしに
                // 削除すると他プロジェクトのコンテナ（停止中の resume 可能な状態を
                // 含む）を巻き込んで壊しうる。確認を取れない以上、削除せずエラーで
                // 中断する。この時点ではイメージビルドが既に完了しているため、
                // その旨をメッセージに含める。
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
                runtime.remove_container(&container.name).await?;
            }
            println!("  Removed {} container(s).", containers.len());
        } else {
            // ユーザーがコンテナ削除を拒否 → config を更新しない（旧コンテナが旧イメージのまま残る）
            eprintln!(
                "  Skipping config update: existing containers were not removed. \
                 Re-run `vibepod init` and remove containers to apply the new image."
            );
            return Ok(());
        }
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
}
