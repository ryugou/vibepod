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

    if opts.worktree && opts.prompt.is_none() {
        bail!("--worktree requires --prompt");
    }
    if opts.worktree && opts.template.is_some() {
        // --worktree は毎回ランダム名の使い捨てコンテナを作る一方、
        // --template は (project, template) ごとに永続コンテナを作る。
        // この 2 つは命名モデルが衝突するため Phase 2 では同時指定を
        // 禁止する。template 対応の worktree モードは将来の phase で検討。
        bail!("--worktree and --template cannot be used together in Phase 2");
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

    banner::print_banner();
    if opts.prompt.is_some() {
        println!();
        println!("Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("Remote: {}", r);
        }
        println!("Branch: {}", branch);
        println!();
    } else {
        println!("  ┌");
        println!("  │");
        println!("  ◇  Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("  │  Remote: {}", r);
        }
        println!("  │  Branch: {}", branch);
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

    // Load vibepod project config
    let config_dir_early = config::default_config_dir()?;
    let vibepod_config = config::VibepodConfig::load(&cwd, &config_dir_early)?;

    // Language detection
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

    // 2. Load config
    let config_dir = config::default_config_dir()?;
    let global_config = config::load_global_config(&config_dir)?;

    // Note: 埋め込み template の展開はここでは**行わない**。展開は
    // template mode が使われ、かつ要求された template が user-provided
    // として解決できない時だけ遅延実行する（step 4 の container_name
    // 計算内）。`vibepod template list` / `template set-default` は
    // 列挙のみで write を行わないため、host mode 専用ユーザーの
    // read-only `~/.config/vibepod/` setup を壊さない。

    // 3. Check Docker & image
    let runtime = DockerRuntime::new()
        .await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;

    if !runtime.image_exists(&global_config.image).await? {
        bail!(
            "Docker image '{}' not found. Run `vibepod init` first.",
            global_config.image
        );
    }

    // 4. Compute container name
    // --worktree: ランダムハッシュ（使い捨て）
    // それ以外:
    //   - host mode (--template 未指定): プロジェクトパスの SHA256 先頭 8 文字（v1.4.3 互換）
    //   - template mode (--template <name>): プロジェクトパス + template の canonical path の SHA256
    //
    // template 指定時だけ hash を変えることで、template モードは host
    // mode とは別の container に落ちつつ、既存の host mode container は
    // v1.4.3 と同じ名前のままで互換性を保つ。
    //
    // template mode の hash 入力には **canonical path** を使う。これで
    // macOS のような case-insensitive FS で `--template review` と
    // `--template Review` が同じディレクトリに解決される場合に同じ container
    // に落ちる（raw 名を使うと 2 つの container に split されてしまう）。
    // 同時に、resolve_template_dir が symlink escape を防ぐので path
    // traversal の心配もない。
    let effective_template =
        super::template::effective_template_name(opts, &vibepod_config, &config_dir);
    let container_name = if opts.worktree {
        let short_hash: String = (0..6)
            .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
            .collect();
        format!("vibepod-{}-{}", project_name, short_hash)
    } else if let Some(ref tmpl) = effective_template {
        // まず要求された template をそのまま resolve する。user-provided
        // の template は embedded extraction に依存せず、read-only
        // `~/.config/vibepod/` でもそのまま使えるべき。初回で resolve
        // できなかった場合、**かつ要求された名前が実際に embedded 集合に
        // 存在する** 場合のみ embedded 展開を試みて再 resolve する。
        // これにより:
        //   - typo (`--template rustcod`) による書き込み発生 & 誤解を招く
        //     エラーメッセージ (`Failed to create templates root`) を防ぐ
        //   - user-provided template だけを期待する read-only setup で
        //     typo が無駄な mutation を起こさない
        //   - 素直な「Template 'rustcod' not found」エラーで返す
        let canonical_template = match super::template::resolve_template_dir(tmpl, &config_dir) {
            Ok(path) => path,
            Err(first_err) => {
                // extract gate は **厳密一致** のみ許容する。
                // case-insensitive 比較だと Linux (case-sensitive FS) で
                // `--template Review` が「embedded 名に該当する」と判定
                // されて不要な extraction (または read-only setup での
                // write エラー) を引き起こすため。
                //
                // macOS のような case-insensitive FS では、正しい大文字
                // 小文字 (`--template review`) で 1 度でも実行すれば
                // extraction が走り、以降 `Review` 等の case variant は
                // resolve_template_dir の FS-level case-insensitive 解決
                // で通るようになる。初回の miscased 呼び出しは「Template
                // 'Review' not found」で素直に返すのが正しい挙動。
                let is_embedded = super::template::embedded_template_names()
                    .iter()
                    .any(|n| n == tmpl);
                if !is_embedded {
                    // user-provided 前提で resolve 失敗: そのまま元のエラーを返す。
                    // extract を呼ばないので `~/.config/vibepod/templates/` は
                    // 作られない。
                    return Err(first_err);
                }
                // 要求が embedded 名: lazy extract して再 resolve。
                super::template::extract_embedded_templates_if_missing(&config_dir)?;
                super::template::resolve_template_dir(tmpl, &config_dir)?
            }
        };
        let hash_input = format!("{}|template={}", cwd_str, canonical_template.display());
        let hash = path_hash_8(&hash_input);
        format!("vibepod-{}-{}", project_name, hash)
    } else {
        // Host mode: v1.4.3 互換の hash 計算を維持
        let hash = path_hash_8(&cwd_str);
        format!("vibepod-{}-{}", project_name, hash)
    };

    // 5. Check container status and handle --new flag
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

    // 6. 既存コンテナのラベルを取得（設定変更の検知に使用）
    // env ファイルのパースより前に取得し、env ハッシュとの比較は step 9 後に行う
    let stored_labels_opt = if container_status != ContainerStatus::None && !opts.worktree {
        Some(runtime.get_container_labels(&container_name).await?)
    } else {
        None
    };

    // 7. Project registration
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

    // 8. Build claude args
    let mut claude_args: Vec<String> = Vec::new();
    if !interactive {
        claude_args.push("--dangerously-skip-permissions".to_string());
    }
    if opts.resume {
        claude_args.push("--resume".to_string());
    }
    if let Some(ref p) = opts.prompt {
        claude_args.push("-p".to_string());
        claude_args.push(p.clone());
        claude_args.push("--output-format".to_string());
        claude_args.push("stream-json".to_string());
        claude_args.push("--verbose".to_string());
    }

    // 9. Resolve env file if provided
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

    // 9b. 設定変更の検知（env ファイル解決後に env ハッシュを含めて比較）
    // vibepod v2 の mode 切り替え mechanism: --template 指定時は template
    // mount、未指定時は従来の host mount を使う。`effective_template` は
    // 既に container_name 計算時 (step 4) に resolve 済み。
    let home = crate::config::home_dir()?;

    // claude_config_mounts / host_settings_exists は label 計算（9b）と
    // extra_mounts 構築（下部）の両方で共有される。1 度だけ計算して使い回す。
    let (claude_config_mounts, host_settings_exists) =
        if let Some(ref template_name) = effective_template {
            // template mode: vibepod 管理の template 配下のみ。
            // host の sanitized settings.json はマウントしない
            let mounts = super::template::build_template_mounts(template_name, &config_dir)?;
            (mounts, false)
        } else {
            // host mode: v1.4.3 互換挙動
            let mounts = super::build_claude_config_mounts(&home);
            let exists = home.join(".claude").join("settings.json").is_file();
            (mounts, exists)
        };

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
        mounts_parts.sort();

        let current_lang = lang_display
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();

        // env ファイル解決後の resolved_env_vars をハッシュ化（env ファイルの変更も検知）
        let current_env_hash = hash_env_vars(&resolved_env_vars);

        let mut current_labels = std::collections::HashMap::new();

        current_labels.insert("vibepod.mounts".to_string(), mounts_parts.join("|"));
        current_labels.insert("vibepod.network".to_string(), opts.no_network.to_string());
        current_labels.insert("vibepod.lang".to_string(), current_lang);
        current_labels.insert("vibepod.env_hash".to_string(), current_env_hash);

        // Template mode では mount set の変更は **critical**。
        // docker のバインドマウントはコンテナ作成時に固定されるため、
        // template に CLAUDE.md / skills/ / agents/ / plugins/ /
        // settings.json を追加・削除しても既存コンテナには反映されない。
        // warn_config_changes は通常の非致命的な変更（lang / env 等）の
        // 警告表示用なので、template mode の mount 変更はそれより前に
        // hard fail させる。
        if effective_template.is_some() {
            let stored_mounts_raw = stored_labels
                .get("vibepod.mounts")
                .map(|s| s.as_str())
                .unwrap_or("");
            let current_mounts_raw = current_labels
                .get("vibepod.mounts")
                .map(|s| s.as_str())
                .unwrap_or("");
            // warn_config_changes と同じ legacy 正規化ロジックを適用
            let stored_mounts = normalize_mounts_label_legacy(stored_mounts_raw);
            if stored_mounts != current_mounts_raw {
                bail!(
                    "Template mount set changed since this container was created \
                     (template: '{}'). Bind mounts are fixed at container creation \
                     time, so edits that add or remove files/directories in the \
                     template (CLAUDE.md, skills/, agents/, plugins/, settings.json) \
                     cannot take effect on the existing container. Run with --new to \
                     recreate the container with the updated mount set.",
                    effective_template.as_deref().unwrap_or("")
                );
            }
        }

        warn_config_changes(&stored_labels, &current_labels)?;
    }

    // 10. Auth: load token
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

    // Per-container runtime directory: all vibepod-managed runtime files for
    // this container (temp claude.json copy, sanitized settings.json, etc.)
    // live under ~/.config/vibepod/runtime/<container_name>/. Created up-front
    // so disposable cleanup can target it unconditionally regardless of which
    // artifacts ended up being created.
    let runtime_dir = config_dir.join("runtime").join(&container_name);
    std::fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("Failed to create runtime dir: {}", runtime_dir.display()))?;

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

    // Parse --mount arguments
    let mut extra_mounts = Vec::new();
    for arg in &opts.mount {
        let parsed =
            parse_mount_arg(arg).with_context(|| format!("Invalid --mount argument: {}", arg))?;
        extra_mounts.push(parsed);
    }

    // `claude_config_mounts` は 9b の分岐で既に解決済み（template mode なら
    // template mount、host mode なら host mount）。ここで再計算せずに
    // そのまま `extra_mounts` に積む。
    for (host, container) in &claude_config_mounts {
        extra_mounts.push((host.clone(), container.clone()));
    }

    // Host mode の場合だけ sanitized settings.json を実際に生成してマウント
    // に追加する。template mode では host の settings.json は触らない。
    if effective_template.is_none() {
        if let Some((host, container)) =
            super::prepare_sanitized_settings_mount(&home, &config_dir, &container_name)?
        {
            extra_mounts.push((host, container));
        }
    }

    Ok(Some(RunContext {
        container_name,
        effective_workspace,
        claude_args,
        resolved_env_vars,
        exec_env_vars,
        setup_cmd,
        temp_claude_json,
        runtime_dir,
        global_config,
        home,
        worktree_branch_name,
        worktree_dir_name,
        lang_display,
        store,
        deferred_session,
        extra_mounts,
        container_status,
        is_disposable,
        no_network: opts.no_network,
        prompt_idle_timeout: vibepod_config.prompt_idle_timeout(),
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
