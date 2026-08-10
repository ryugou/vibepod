use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::process::Command;

use crate::config::{self, ProjectEntry};
use crate::git;
use crate::runtime::{ContainerStatus, DockerRuntime};
use crate::session::{self, SessionStore};
use crate::ui::{banner, prompts};

use super::{
    detect_languages, get_lang_install_cmd, hash_env_vars, parse_mount_arg, RunContext, RunOptions,
};

/// `profile = "swift"` のコンテナでエージェントへ渡す環境情報。
///
/// バージョン番号を書かない: 正本は `templates/Dockerfile` の ARG であり、
/// ここへ書き写すとイメージ更新のたびに二重管理になる。バージョンが必要な
/// 場合、エージェントはコンテナ内で `swift --version` を実行できる。
const SWIFT_AVAILABLE_PREAMBLE: &str = "[vibepod 環境情報 / 自動付与]
このコンテナには Swift toolchain と SwiftLint が導入済みで、すぐに使える。
- 検証はコンテナ内で実行すること(swift build / swift test / swiftlint lint)。
- toolchain の追加導入は不要。試みてはならない。
- Linux 環境のため、Apple フレームワーク(CryptoKit / SwiftUI / UIKit 等)に依存する
  ターゲットはビルドできない。対象を Foundation のみに依存するパッケージへ限定すること。
- コンテナ内が green でも macOS 側の検証を代替しない。

--- ここから利用者のプロンプト ---";

/// `Package.swift` があるのに profile 未指定のコンテナでエージェントへ渡す
/// 環境情報。自力導入は共有ライブラリ不足で必ず失敗するため、試行そのものを
/// 禁じたうえで恒久対応(config.toml への profile 設定)を示す。
const SWIFT_ABSENT_PREAMBLE: &str = "[vibepod 環境情報 / 自動付与]
このコンテナに Swift toolchain と SwiftLint は導入されていない。
- インストールを試みてはならない。共有ライブラリ不足で失敗し、時間だけを消費する。
- ビルド・テスト・lint は実行せず、最終出力に「未実行」と明記すること。
- 恒久対応: .vibepod/config.toml の [run] へ profile = \"swift\" を設定する。

--- ここから利用者のプロンプト ---";

/// profile と workspace の状態から、エージェントへ渡す環境情報ブロックを
/// 導出する。前置が不要な場合は `None` を返す。
///
/// 生成規則(設計 3.3):
///
/// | `profile`       | `Package.swift` | 戻り値                     |
/// | --------------- | --------------- | --------------------------- |
/// | `Some("swift")` | 問わない        | `SWIFT_AVAILABLE_PREAMBLE` |
/// | `None`          | あり            | `SWIFT_ABSENT_PREAMBLE`    |
/// | `None`          | なし            | `None`                     |
///
/// `VALID_PROFILES` へ `swift` 以外を追加する場合は、この関数の分岐と対応する
/// 定数を同時に追加すること(追加しない限り新 profile は `None` を返す)。
pub fn environment_preamble(profile: Option<&str>, has_package_swift: bool) -> Option<String> {
    match (profile, has_package_swift) {
        (Some("swift"), _) => Some(SWIFT_AVAILABLE_PREAMBLE.to_string()),
        (None, true) => Some(SWIFT_ABSENT_PREAMBLE.to_string()),
        _ => None,
    }
}

/// Claude CLI に渡す引数列を組み立てる。
///
/// `prepare_context` 内の Docker チェックなどの副作用から独立させるため
/// 純関数として切り出している。ユニットテストから直接検証できる。
///
/// - `interactive = true`（`--prompt` なし・`--resume` なし）のときは
///   パーミッションバイパスを付けない（ユーザーが対話的に承認する想定）。
/// - 非対話モード（`--prompt` / `--resume`）では常に
///   `--dangerously-skip-permissions` を付与する。コンテナに閉じ込めた
///   確認なし実行が vibepod の主目的であり、承認者不在で自律実行するため。
/// - `--resume` や `-p <prompt>`（`--output-format stream-json --verbose`
///   付き）は従来通り後段で積み上げる。
/// - `preamble` が `Some` かつ `opts.prompt` が `Some` のとき、`-p` の値を
///   `<preamble>\n<prompt>` とする。前置はこの引数列にのみ現れ、ロックキー・
///   `Session.prompt`・ログ表示は元のプロンプトのままとする（設計 3.5）。
pub fn build_claude_args(
    opts: &RunOptions,
    interactive: bool,
    preamble: Option<&str>,
) -> Vec<String> {
    let mut claude_args: Vec<String> = Vec::new();
    if !interactive {
        claude_args.push("--dangerously-skip-permissions".to_string());
    }
    // `--model` は対話・非対話の両パスで有効。vibepod は値を検証せず、
    // そのまま `claude --model <name>` に渡す（正当性判断は claude 側）。
    if let Some(ref model) = opts.model {
        claude_args.push("--model".to_string());
        claude_args.push(model.clone());
    }
    if opts.resume {
        claude_args.push("--resume".to_string());
    }
    if let Some(ref p) = opts.prompt {
        claude_args.push("-p".to_string());
        claude_args.push(match preamble {
            Some(pre) => format!("{pre}\n{p}"),
            None => p.clone(),
        });
        claude_args.push("--output-format".to_string());
        claude_args.push("stream-json".to_string());
        claude_args.push("--verbose".to_string());
    }
    claude_args
}

/// プロジェクトパスの SHA256 先頭 8 文字（hex）を返す。
fn path_hash_8(path: &str) -> String {
    let hash = Sha256::digest(path.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
    hex[..8].to_string()
}

/// v1.4.3 未満で作成されたコンテナは、サニタイズ済み settings.json マーカーを
/// `:/home/vibepod/.claude/settings.json` という `host:container` 形式の空 host
/// で保存していた。v1.4.3 以降は `sanitized_settings=/home/vibepod/.claude/settings.json`
/// という専用 prefix 形式に変更している。後方互換のため、比較前に旧形式を新形式へ
/// 正規化する。
fn normalize_mounts_label_legacy(raw: &str) -> String {
    raw.split('|')
        .map(|part| {
            if part == ":/home/vibepod/.claude/settings.json" {
                super::SANITIZED_SETTINGS_LABEL_MARKER
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// 設定ラベルの差分を検出して警告を表示する。
fn warn_config_changes(
    stored: &std::collections::HashMap<String, String>,
    current: &std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    // ネットワーク設定の変更を確認: --no-network が要求されているが既存コンテナにはない場合はエラー
    let stored_network = stored
        .get("vibepod.network")
        .map(|s| s.as_str())
        .unwrap_or("false");
    let current_network = current
        .get("vibepod.network")
        .map(|s| s.as_str())
        .unwrap_or("false");
    if current_network == "true" && stored_network != "true" {
        anyhow::bail!(
            "Network isolation (--no-network) was requested but the existing container was \
             created with network access. Run with --new to recreate the container with the \
             correct network configuration."
        );
    }

    let mut changes: Vec<String> = Vec::new();

    for key in &[
        "vibepod.lang",
        "vibepod.profile",
        "vibepod.network",
        "vibepod.mounts",
        "vibepod.env_hash",
    ] {
        let label_name = key.strip_prefix("vibepod.").unwrap_or(key);
        let raw_stored = stored.get(*key).map(|s| s.as_str()).unwrap_or("");
        let raw_current = current.get(*key).map(|s| s.as_str()).unwrap_or("");
        // vibepod.mounts だけ、stored 側（既存コンテナに記録されている旧形式）の
        // みを新形式に正規化する。current 側も正規化してしまうと、ユーザーが
        // `--mount :/home/vibepod/.claude/settings.json` のように空ホストで
        // マウント指定した場合に意図せずマーカーへ置換され、設定変更の検知が
        // マスクされるため。
        let (stored_val, current_val): (String, String) = if *key == "vibepod.mounts" {
            (
                normalize_mounts_label_legacy(raw_stored),
                raw_current.to_string(),
            )
        } else {
            (raw_stored.to_string(), raw_current.to_string())
        };
        if stored_val != current_val {
            changes.push(format!(
                "{}: {} → {}",
                label_name,
                if stored_val.is_empty() {
                    "(none)".to_string()
                } else {
                    stored_val
                },
                if current_val.is_empty() {
                    "(none)".to_string()
                } else {
                    current_val
                }
            ));
        }
    }

    if !changes.is_empty() {
        eprintln!(
            "Warning: Container configuration has changed ({}).",
            changes.join(", ")
        );
        eprintln!("Run with --new to recreate the container.");
        eprintln!("Continuing with existing container...");
    }

    Ok(())
}

pub(super) async fn prepare_context(opts: &RunOptions) -> Result<Option<RunContext>> {
    // 実時間上限を先に解釈して fail-fast する（コンテナ作成やイメージ
    // ビルドの前に不正な --timeout を弾く）。未指定時は既定値。
    let overall_timeout = match &opts.timeout {
        Some(raw) => super::parse_timeout_secs(raw)?,
        None => super::DEFAULT_OVERALL_TIMEOUT_SECS,
    };

    let interactive = !opts.resume && opts.prompt.is_none();

    // 1. Check git repo
    let cwd = std::env::current_dir()?;
    if !git::is_git_repo(&cwd) {
        bail!("Not a git repository. Run this command inside a git-initialized directory.");
    }
    // シンボリックリンクや `.` を解決して安定したパス文字列を得る
    // コンテナ名ハッシュの元になるため、パス表記の違いで異なるコンテナが作られないよう正規化する
    let cwd_canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let cwd_str = cwd_canonical.to_string_lossy().to_string();

    // Load vibepod project config and validate `profile` before any
    // repository-mutating side effect (in particular `--worktree`'s
    // `git worktree add` + branch creation below). Both calls depend only
    // on `cwd`, which is already resolved above. Validating here — rather
    // than after worktree creation — keeps the existing fail-fast contract
    // (same as the `--timeout` check above, which runs before any Docker
    // work): an invalid `profile` in `.vibepod/config.toml` must abort
    // before we create a worktree/branch, otherwise a `git worktree add`
    // failure path leaves an orphaned worktree directory and branch behind
    // that require manual `git worktree remove` + `git branch -D` cleanup.
    let config_dir = config::default_config_dir()?;
    let vibepod_config = config::VibepodConfig::load(&cwd, &config_dir)?;

    // Profile: 設定ファイル専用（CLI フラグなし）。無効値は起動前に fail-fast
    // させ、有効な選択肢をメッセージに含める（設計書 2.1）。
    let effective_profile = vibepod_config.profile();
    config::validate_profile(&effective_profile)?;

    // 起動出力（設計 2）と Session 記録（設計 4）が effective_image を参照する
    // ため、global config の読み込みとイメージ名の算出をここへ前倒しする。
    // イメージの自動ビルド（ensure_image_available）は現在位置に残す —
    // ビルド所要時間だけ Session.started_at が後ろへずれるのを避けるため。
    let global_config = config::load_global_config(&config_dir)?;
    // profile 未指定時は現行どおり global_config.image をそのまま使う。
    let effective_image = match effective_profile.as_deref() {
        Some(profile) => config::image_for_profile(&global_config.image, profile),
        None => global_config.image.clone(),
    };

    if opts.worktree && opts.prompt.is_none() {
        bail!("--worktree requires --prompt");
    }

    // Record session for restore
    let head_before = git::get_head_hash(&cwd)?;
    let current_branch = git::get_current_branch(&cwd).unwrap_or_else(|_| "unknown".to_string());

    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir.clone());

    // Proactively create .worktrees/ (unconditionally, even without --worktree)
    // so that vibepod's --worktree feature and any tooling that expects the
    // directory can rely on its existence without prompting the user later.
    let worktrees_dir = cwd.join(".worktrees");
    if !worktrees_dir.exists() {
        std::fs::create_dir_all(&worktrees_dir)?;
    }

    // Ensure .vibepod/ and .worktrees/ are in .gitignore
    let gitignore_path = cwd.join(".gitignore");
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        let needs_vibepod = !content
            .lines()
            .any(|l| l.trim() == ".vibepod/" || l.trim() == ".vibepod");
        let needs_worktrees = !content
            .lines()
            .any(|l| l.trim() == ".worktrees/" || l.trim() == ".worktrees");
        if needs_vibepod || needs_worktrees {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&gitignore_path)?;
            use std::io::Write;
            if needs_vibepod {
                writeln!(file, "\n.vibepod/")?;
            }
            if needs_worktrees {
                writeln!(file, "\n.worktrees/")?;
            }
        }
    } else {
        std::fs::write(&gitignore_path, ".vibepod/\n.worktrees/\n")?;
    }

    let prompt_label = if interactive {
        "interactive".to_string()
    } else if opts.resume {
        "--resume".to_string()
    } else {
        opts.prompt.as_deref().unwrap_or("").to_string()
    };

    // Session recording is deferred until the container actually starts.
    let session_id = session::generate_session_id();
    let deferred_session = session::Session {
        id: session_id.clone(),
        started_at: chrono::Local::now().to_rfc3339(),
        head_before,
        branch: current_branch.clone(),
        prompt: prompt_label,
        claude_session_path: None,
        restored: false,
        image: Some(effective_image.clone()),
        profile: effective_profile.clone(),
    };

    // プロジェクト名はシンボリックリンク解決後のパスから取得する
    // ハッシュも正規化パスから計算するため、両方が一致しないと symlink 経由アクセス時に
    // 異なるコンテナが作られてしまう
    let project_name = cwd_canonical
        .file_name()
        .context("Cannot determine project name")?
        .to_string_lossy()
        .to_string();

    // Get remote URL (optional)
    let remote = git::get_remote_url(&cwd);

    // Get branch
    let branch = current_branch;

    // profile 未指定を "default" と表記する。行の有無で判別させないため、
    // profile の指定有無にかかわらず常に出力する（設計 2.2）。
    let profile_label = effective_profile.as_deref().unwrap_or("default");
    banner::print_banner();
    if opts.prompt.is_some() {
        println!();
        println!("Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("Remote: {}", r);
        }
        println!("Branch: {}", branch);
        println!("Profile: {} (image: {})", profile_label, effective_image);
        println!();
    } else {
        println!("  ┌");
        println!("  │");
        println!("  ◇  Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("  │  Remote: {}", r);
        }
        println!("  │  Branch: {}", branch);
        println!(
            "  │  Profile: {} (image: {})",
            profile_label, effective_image
        );
        println!("  │");
    }

    // Worktree creation
    // --worktree: 使い捨てコンテナ（実行後削除）、コンテナ名はランダムハッシュ
    let is_disposable = opts.worktree;
    let (effective_workspace, worktree_branch_name, worktree_dir_name) = if opts.worktree {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let branch_name = format!("vibepod/prompt-{}", ts);
        let dir_name = format!("vibepod-prompt-{}", ts);
        let wt_path = cwd.join(".worktrees").join(&dir_name);

        let wt_path_str = wt_path.to_string_lossy().to_string();
        let output = Command::new("git")
            .args(["worktree", "add", &wt_path_str, "-b", &branch_name])
            .current_dir(&cwd)
            .output()
            .context("Failed to run git worktree")?;

        if !output.status.success() {
            bail!(
                "Failed to create worktree: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        println!("Created worktree: .worktrees/{}", dir_name);
        println!("Branch: {}", branch_name);

        (
            wt_path.to_string_lossy().to_string(),
            Some(branch_name),
            Some(dir_name),
        )
    } else {
        (cwd_str.clone(), None, None)
    };

    // F11（フル再レビュー指摘）: この Package.swift 検知は、すぐ下の
    // `detect_languages` とは意図的に別立てにしている。`detect_languages` は
    // `--lang` 表示ラベルや `get_lang_install_cmd` によるコンテナ内インストール
    // コマンドの機構に載るが、Swift は profile 経由のイメージ選定（2.2 節。
    // Swift toolchain 自体は Dockerfile に焼き込み済みで、apt 等でその場
    // インストールしない）で扱う言語であり、この2つの仕組みに swift を
    // 混ぜたくないため。
    //
    // 判定対象を `cwd` ではなく `effective_workspace` にしているのは、
    // `--worktree` 実行時に Package.swift の有無を実際の作業先（worktree 内）
    // で判定すべきという設計書 2.5 手順4の要件に従うため。cwd で判定すると、
    // worktree 内にのみ Package.swift が存在するケースを見逃す。
    //
    // profile 未指定かつ workspace 直下に Package.swift があるプロジェクトへ、
    // profile 設定を促す 1 行の注意を出す。実行は継続する（設計書 2.5 手順4）。
    let has_package_swift = std::path::Path::new(&effective_workspace)
        .join("Package.swift")
        .is_file();
    if effective_profile.is_none() && has_package_swift {
        eprintln!(
            "Note: Detected Package.swift but no `profile` is set. Add `profile = \"swift\"` \
             under [run] in .vibepod/config.toml to use the Swift toolchain image."
        );
    }

    // Language detection: `--lang` > project/global config `lang` > cwd
    // auto-detect. The selected languages drive the in-container setup
    // command (toolchain install).
    let effective_lang = opts.lang.clone().or_else(|| vibepod_config.lang());
    let detected_langs: Vec<(String, &'static str)> = if effective_lang.is_none() {
        detect_languages(&cwd)
    } else {
        Vec::new()
    };

    let (lang_names, lang_display): (Vec<String>, String) = if let Some(ref l) = effective_lang {
        (vec![l.clone()], format!("{} (--lang)", l))
    } else if detected_langs.len() == 1 {
        let (name, file) = &detected_langs[0];
        (
            vec![name.clone()],
            format!("{} (detected from {})", name, file),
        )
    } else if detected_langs.len() > 1 {
        let names: Vec<String> = detected_langs.iter().map(|(n, _)| n.clone()).collect();
        let display = format!("{} (auto-detected)", names.join(", "));
        (names, display)
    } else {
        (Vec::new(), String::new())
    };

    let setup_cmd: Option<String> = {
        let setup_parts: Vec<String> = lang_names
            .iter()
            .filter_map(|l| get_lang_install_cmd(l).map(|s| s.to_string()))
            .collect();
        if setup_parts.is_empty() {
            None
        } else {
            Some(setup_parts.join(" && "))
        }
    };

    if setup_cmd.is_some() {
        eprintln!("Note: Language/tool setup requires sudo in the container. If setup fails, run `vibepod init` to rebuild the image.");
    }

    // 2. Check Docker & image
    let runtime = DockerRuntime::new()
        .await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;

    // イメージが無ければ（既定で）自動ビルドして処理を継続する。これが
    // 「他セッションで前準備なしにすぐ使える」を実現する要。並行実行の
    // 二重ビルドはビルドロックで直列化する（ensure_image_available 内）。
    crate::cli::init::ensure_image_available(
        &runtime,
        &effective_image,
        &config_dir,
        opts.no_auto_build,
        effective_profile.as_deref(),
    )
    .await?;

    // 3. Compute container name
    //   - worktree: random short hash (disposable)
    //   - otherwise: project path → SHA256[:8] (v1.4.3 compatible)
    let container_name = if opts.worktree {
        let short_hash: String = (0..6)
            .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
            .collect();
        format!("vibepod-{}-{}", project_name, short_hash)
    } else {
        let hash = path_hash_8(&cwd_str);
        format!("vibepod-{}-{}", project_name, hash)
    };

    // 4. Check container status and handle --new flag
    let mut container_status = if opts.worktree {
        // ワークツリーはランダム名なので常に None
        ContainerStatus::None
    } else {
        runtime.find_container_status(&container_name).await?
    };

    if opts.new_container {
        match container_status {
            ContainerStatus::Running => {
                bail!("Container is running. Stop it with `vibepod stop` or `vibepod rm` first.");
            }
            ContainerStatus::Stopped => {
                runtime.remove_container(&container_name).await?;
                container_status = ContainerStatus::None;
            }
            ContainerStatus::None => {}
        }
    }

    // 5. 既存コンテナのラベルを取得（設定変更の検知に使用）
    // env ファイルのパースより前に取得し、env ハッシュとの比較は step 8 後に行う
    let stored_labels_opt = if container_status != ContainerStatus::None && !opts.worktree {
        Some(runtime.get_container_labels(&container_name).await?)
    } else {
        None
    };

    // 6. Project registration
    let mut projects = config::load_projects(&config_dir)?;
    let should_register = if !config::is_project_registered(&projects, &cwd_str) {
        if interactive {
            prompts::confirm_project_registration(&project_name)?
        } else {
            true // Auto-register in non-interactive mode
        }
    } else {
        false
    };
    if should_register {
        config::register_project(
            &mut projects,
            ProjectEntry {
                name: project_name.clone(),
                path: cwd_str.clone(),
                remote: remote.clone(),
                registered_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        config::save_projects(&projects, &config_dir)?;
    }

    // 7. Build claude args
    // コンテナ内エージェントへ環境を伝える経路は claude -p の引数のみ
    // （設計 3.5）。ロックキー・Session.prompt・ログ表示は元のプロンプトを使う。
    let preamble = environment_preamble(effective_profile.as_deref(), has_package_swift);
    let claude_args = build_claude_args(opts, interactive, preamble.as_deref());

    if std::env::var("VIBEPOD_TRACE").is_ok() {
        eprintln!("vibepod: claude_args = {:?}", claude_args);
    }

    // 8. Resolve env file if provided
    let mut resolved_env_vars = opts.env_vars.clone();
    if let Some(ref env_file_path) = opts.env_file {
        let content = std::fs::read_to_string(env_file_path)
            .with_context(|| format!("Failed to read env file: {}", env_file_path))?;

        let parsed: Vec<(String, String)> = content
            .lines()
            .filter(|line| {
                let t = line.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .filter_map(|line| {
                let t = line.trim();
                let (key, value) = t.split_once('=')?;
                let value = value.trim_matches('"').trim_matches('\'');
                Some((key.to_string(), value.to_string()))
            })
            .collect();

        let has_op_refs = parsed.iter().any(|(_, v)| v.starts_with("op://"));

        if has_op_refs {
            // Use `op run` to resolve op:// references
            let op_available = Command::new("op")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !op_available {
                bail!(
                    "env file contains op:// references but 1Password CLI (op) is not installed.\n  \
                     Install it: https://developer.1password.com/docs/cli/"
                );
            }

            println!("  ◇  Resolving op:// references via 1Password CLI...");

            let output = Command::new("op")
                .args([
                    "run",
                    &format!("--env-file={}", env_file_path),
                    "--no-masking",
                    "--",
                    "env",
                ])
                .output()
                .context("Failed to run `op run` to resolve secrets")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("1Password CLI failed to resolve secrets: {}", stderr);
            }

            // Parse resolved env output — only keep keys that were in our env file
            let env_keys: std::collections::HashSet<String> =
                parsed.iter().map(|(k, _)| k.clone()).collect();
            let resolved_output = String::from_utf8_lossy(&output.stdout);
            for line in resolved_output.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if env_keys.contains(key) {
                        resolved_env_vars.push(format!("{}={}", key, value));
                    }
                }
            }
        } else {
            // No op:// references, pass as-is
            for (key, value) in &parsed {
                resolved_env_vars.push(format!("{}={}", key, value));
            }
        }
    }

    // 8b. 設定変更の検知（env ファイル解決後に env ハッシュを含めて比較）
    let home = crate::config::home_dir()?;

    // Per-container runtime directory: holds the temp claude.json copy and
    // the sanitized settings.json written below. Created here so cleanup of
    // disposable containers can remove the whole directory unconditionally.
    let runtime_dir = config_dir.join("runtime").join(&container_name);
    std::fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("Failed to create runtime dir: {}", runtime_dir.display()))?;

    // Mount the host's `~/.claude/` allowlist (CLAUDE.md / agents / skills /
    // specs / plugins) read-only. `claude_config_mounts` / `host_settings_exists`
    // are shared by the label computation (9b) and the extra_mounts assembly
    // below, so they are computed once here and reused.
    let claude_config_mounts = super::build_claude_config_mounts(&home);
    let host_settings_exists = home.join(".claude").join("settings.json").is_file();
    // auth.json の有無判定は prepare_codex_mount 内の has_auth 判定と完全に
    // 同じ基準にする(config.toml の有無は問わない)。基準がずれると、実際には
    // bind mount されない codex を「ある」と誤ってラベルに含めてしまい、
    // 構成差分の警告が出なくなる。この判定はホストの生 `~/.codex/` を見て
    // いるだけなので、round 10 で auth store / per-container ステージの
    // 2 段構成に変わった後もそのまま通用する。
    let host_codex_dir = home.join(".codex");
    let host_codex_auth_exists = super::host_codex_stage_entries(&host_codex_dir)
        .with_context(|| {
            format!(
                "Failed to enumerate host codex directory {}",
                host_codex_dir.display()
            )
        })?
        .iter()
        .any(|(_, name)| *name == "auth.json");

    if let Some(stored_labels) = stored_labels_opt {
        let mut mounts_parts: Vec<String> = Vec::new();
        for arg in &opts.mount {
            if let Ok((h, c)) = parse_mount_arg(arg) {
                mounts_parts.push(format!("{}:{}", h, c));
            }
        }
        for (h, c) in &claude_config_mounts {
            mounts_parts.push(format!("{}:{}", h, c));
        }
        // Sanitized settings: include only container destination in the label so
        // regenerated host-side runtime files do not trigger a spurious recreate.
        // Use a dedicated prefix (SANITIZED_SETTINGS_LABEL_MARKER) so this
        // label-only marker cannot collide with a user-provided mount serialized
        // as "{host}:{container}".
        if host_settings_exists {
            mounts_parts.push(super::SANITIZED_SETTINGS_LABEL_MARKER.to_string());
        }

        // Encode the FULL sorted lang_names set so reuse re-provisions
        // whenever any language is added or removed.
        let current_lang = {
            let mut names = lang_names.clone();
            names.sort();
            names.dedup();
            names.join(",")
        };

        // env ファイル解決後の resolved_env_vars をハッシュ化（env ファイルの変更も検知）
        let current_env_hash = hash_env_vars(&resolved_env_vars);

        let mut current_labels = std::collections::HashMap::new();

        current_labels.insert(
            "vibepod.mounts".to_string(),
            super::build_mounts_label(mounts_parts, host_codex_auth_exists),
        );
        current_labels.insert("vibepod.network".to_string(), opts.no_network.to_string());
        current_labels.insert("vibepod.lang".to_string(), current_lang);
        current_labels.insert(
            "vibepod.profile".to_string(),
            effective_profile.clone().unwrap_or_default(),
        );
        current_labels.insert("vibepod.env_hash".to_string(), current_env_hash);

        warn_config_changes(&stored_labels, &current_labels)?;
    }

    // 9. Auth: load token
    let auth_manager = crate::auth::AuthManager::new(config_dir.clone());
    let claude_json = home.join(".claude.json");

    let token_data = auth_manager
        .load_token()?
        .context("Not authenticated. Run `vibepod login` first.")?;

    if token_data.needs_renewal() {
        bail!("Token expires soon. Please run `vibepod login` to renew.");
    }

    // 認証トークンは exec_env_vars に格納（コンテナ作成時ではなく毎回 exec で渡す）
    let mut exec_env_vars = Vec::new();
    exec_env_vars.push(format!("CLAUDE_CODE_OAUTH_TOKEN={}", token_data.token));

    // GitHub token: gh auth token でホスト側のトークンを自動取得
    if let Ok(output) = Command::new("gh").args(["auth", "token"]).output() {
        if output.status.success() {
            let gh_token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !gh_token.is_empty() {
                exec_env_vars.push(format!("GH_TOKEN={}", gh_token));
            }
        }
    }

    // `runtime_dir` はこの関数の前半（9b）で既に作成済み。ここでは再作成しない。

    // Copy .claude.json to a per-container runtime file so the host file is
    // protected from container writes. Lives alongside any sanitized
    // settings.json under the same per-container runtime dir.
    let temp_claude_json = if claude_json.exists() {
        let temp_path = runtime_dir.join(".claude.json");
        std::fs::copy(&claude_json, &temp_path).with_context(|| {
            format!(
                "Failed to copy {} to {}",
                claude_json.display(),
                temp_path.display()
            )
        })?;
        Some(temp_path)
    } else {
        None
    };

    // codex CLI 認証(~/.codex/auth.json + config.toml)を、host-only の
    // auth store(`<config_dir>/codex-auth/`)経由で per-container ステージ
    // (`<runtime_dir>/codex/`)へコピーする。ステージは disposable 実行の
    // 終了時に runtime_dir ごと削除される想定の使い捨て領域であり、それで
    // 問題ない — 認証情報の永続化は auth store 側が担う(round 10)。
    // auth.json が無ければ None(codex レビューはこのコンテナでは使えないが、
    // vibepod 自体は継続動作する)。
    let codex_dir = super::prepare_codex_mount(&home, &config_dir, &runtime_dir)?;

    // Parse --mount arguments
    let mut extra_mounts = Vec::new();
    for arg in &opts.mount {
        let parsed =
            parse_mount_arg(arg).with_context(|| format!("Invalid --mount argument: {}", arg))?;
        extra_mounts.push(parsed);
    }

    // `claude_config_mounts`（host `~/.claude/` の allowlist マウント）は
    // 8b で既に解決済み。そのまま `extra_mounts` に積む。
    for (host, container) in &claude_config_mounts {
        extra_mounts.push((host.clone(), container.clone()));
    }

    // ホストの settings.json を hooks/statusLine 除去のうえマウントする。
    if let Some((host, container)) =
        super::prepare_sanitized_settings_mount(&home, &config_dir, &container_name)?
    {
        extra_mounts.push((host, container));
    }

    // Normalize lang_names before storing in RunContext so downstream
    // consumers (build_config_labels, future readers) can trust the
    // invariant without re-normalizing. See RunContext field doc.
    let lang_names = {
        let mut names = lang_names;
        names.sort();
        names.dedup();
        names
    };

    Ok(Some(RunContext {
        container_name,
        effective_workspace,
        claude_args,
        resolved_env_vars,
        exec_env_vars,
        setup_cmd,
        temp_claude_json,
        codex_dir,
        runtime_dir,
        config_dir,
        effective_image,
        profile: effective_profile,
        home,
        worktree_branch_name,
        worktree_dir_name,
        lang_display,
        lang_names,
        store,
        deferred_session,
        extra_mounts,
        container_status,
        is_disposable,
        no_network: opts.no_network,
        prompt_idle_timeout: vibepod_config.prompt_idle_timeout(),
        overall_timeout,
        verbose: opts.verbose,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_mounts_label_legacy_rewrites_old_marker() {
        // v1.4.3 未満で作成されたコンテナが持つ旧形式マーカーを新形式に書き換える
        let input =
            "/Users/a/.claude/skills:/home/vibepod/.claude/skills|:/home/vibepod/.claude/settings.json";
        let normalized = normalize_mounts_label_legacy(input);
        assert!(
            normalized.contains(super::super::SANITIZED_SETTINGS_LABEL_MARKER),
            "expected new marker in: {}",
            normalized
        );
        assert!(
            !normalized.contains(":/home/vibepod/.claude/settings.json|")
                && !normalized.ends_with(":/home/vibepod/.claude/settings.json"),
            "old marker should be gone: {}",
            normalized
        );
    }

    #[test]
    fn test_normalize_mounts_label_legacy_preserves_non_marker_entries() {
        // マーカー以外のマウントエントリはそのまま残す
        let input = "/Users/a/.claude/agents:/home/vibepod/.claude/agents";
        let normalized = normalize_mounts_label_legacy(input);
        assert_eq!(normalized, input);
    }

    #[test]
    fn test_normalize_mounts_label_legacy_already_new_format_is_identity() {
        // すでに新形式のラベルは変更しない
        let input = super::super::SANITIZED_SETTINGS_LABEL_MARKER;
        let normalized = normalize_mounts_label_legacy(input);
        assert_eq!(normalized, input);
    }
}
