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
/// `is_terminal` が true（対話端末）のときだけ `prompts::select_agent()` で
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
fn resolve_agent(is_terminal: bool) -> Result<String> {
    if is_terminal {
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
    let agent = resolve_agent(std::io::IsTerminal::is_terminal(&std::io::stdin()))?;

    // 3. Build image
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

    // 3b. `--rebuild` のときだけ、既に docker 上にある profile バリアント
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

    // 4. イメージ再ビルド後に既存のコンテナを全削除する（config 保存前に行う）
    //    running コンテナがある場合は確認プロンプトを表示（非インタラクティブ時は強制削除）
    let containers = runtime.list_vibepod_containers().await?;
    if !containers.is_empty() {
        let running_count = containers
            .iter()
            .filter(|(_, status)| {
                status.starts_with("Up") || status.to_lowercase().contains("running")
            })
            .count();

        let should_remove =
            if running_count > 0 && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                // インタラクティブ: 確認プロンプト
                prompts::confirm_remove_all_containers(containers.len(), running_count)?
            } else {
                // 非インタラクティブまたは停止済みのみ: 強制削除
                if running_count > 0 {
                    eprintln!(
                        "  Warning: Forcibly removing {} running container(s) \
                     (non-interactive mode).",
                        running_count
                    );
                }
                true
            };

        if should_remove {
            println!("  Removing {} existing container(s)...", containers.len());
            for (container_name, _) in &containers {
                runtime.remove_container(container_name).await?;
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

    // 5. Save config（コンテナ削除後に保存することで、削除キャンセル時に旧イメージが残ったまま
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

    // doc comment を参照。`is_terminal = false` 分岐が dialoguer を経由しない
    // ことを固定するテスト。
    #[test]
    fn resolve_agent_non_interactive_defaults_to_claude() {
        // GlobalConfig::default().default_agent（src/config/global.rs）を
        // リテラルで複製せず、プロンプトを呼ばずに同じ値が返ることを検証する。
        let agent = resolve_agent(false).unwrap();
        assert_eq!(agent, GlobalConfig::default().default_agent);
    }
}
