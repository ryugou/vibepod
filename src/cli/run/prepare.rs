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
        "vibepod.template_setup_hash",
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
    if opts.template.is_some() && opts.mode == crate::cli::RunMode::Review {
        anyhow::bail!(
            "--mode review cannot be combined with --template: custom templates \
             define their own behavior (use one or the other)."
        );
    }

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
    let config_dir = config::default_config_dir()?;
    let vibepod_config = config::VibepodConfig::load(&cwd, &config_dir)?;

    // Template resolution (early): we need the template's canonical
    // path *and* its metadata (vibepod-template.toml → required_langs)
    // **before** we compute the language install plan, because the
    // template can declare languages that MUST be installed regardless
    // of what the cwd-detection or --lang would choose.
    //
    // This block does the lazy extract + resolve that Phase 3/4 would
    // otherwise do inline near the container-name hashing step below.
    // We capture the result here and reuse it when computing the
    // container name, so extraction / resolve is not repeated.
    let effective_template =
        super::template::effective_template_name(opts, &vibepod_config, &config_dir);
    let resolved_template: Option<(String, std::path::PathBuf)> = if opts.worktree {
        None
    } else if let Some(ref tmpl) = effective_template {
        // First try resolving the template as-is. User-provided templates
        // must work on read-only ~/.config/vibepod/ without any extraction.
        let canonical = match super::template::resolve_template_dir(tmpl, &config_dir) {
            Ok(path) => path,
            Err(first_err) => {
                // Only trigger embedded extraction on an exact name match
                // (case-sensitive). See the comment in the earlier inline
                // location for the rationale (typo mutation / read-only).
                let is_embedded = super::template::embedded_template_names()
                    .iter()
                    .any(|n| n == tmpl);
                if !is_embedded {
                    return Err(first_err);
                }
                super::template::extract_single_embedded_template_if_missing(&config_dir, tmpl)?;
                super::template::resolve_template_dir(tmpl, &config_dir)?
            }
        };
        Some((tmpl.clone(), canonical))
    } else {
        None
    };

    // Read template metadata (required_langs, etc.) if in template mode.
    //
    // Explicit `--template` is fail-fast: a malformed vibepod-template.toml
    // for a user-chosen template is a clear user-visible error. But
    // when the template came from `default_prompt_template` in config
    // (best-effort default path), we must NOT turn metadata parse
    // failures into fatal run errors — the spec for that code path is
    // "fall back to host mount on any resolution failure". Emit a
    // warning and drop the template so the run continues on host
    // mounts, mirroring `effective_template_name`'s own fallback.
    let template_is_explicit = opts.template.is_some();
    let (resolved_template, template_metadata) = match resolved_template {
        Some((name, canonical)) => match super::template::read_template_metadata(&canonical) {
            Ok(meta) => (Some((name, canonical)), meta),
            Err(e) if !template_is_explicit => {
                eprintln!(
                    "warning: configured default template '{}' has invalid \
                     vibepod-template.toml and will be ignored; falling back \
                     to host mount. Underlying error: {}",
                    name, e
                );
                (None, super::template::TemplateMetadata::default())
            }
            Err(e) => return Err(e),
        },
        None => (None, super::template::TemplateMetadata::default()),
    };
    // Keep `effective_template` (the name) in sync with `resolved_template`.
    // When we drop the default template above, downstream template-mode
    // checks should also see "no template" so the run proceeds on host mounts.
    let effective_template: Option<String> = resolved_template.as_ref().map(|(n, _)| n.clone());

    // Fingerprint of the template's `setup_commands` list. Empty string
    // when no template was selected or when the template declared no
    // setup_commands. Used by:
    //   - build_config_labels (persisted as `vibepod.template_setup_hash`)
    //   - the template-mode reuse gate below (detects drift vs. stored
    //     hash and hard-fails with a `--new` hint).
    // Newlines are disallowed inside entries (validated at parse time)
    // so joining on '\n' is an unambiguous canonical form.
    let template_setup_hash: String = {
        let cmds = &template_metadata.runtime.setup_commands;
        if cmds.is_empty() {
            String::new()
        } else {
            path_hash_8(&cmds.join("\n"))
        }
    };

    // Language detection
    let effective_lang = opts.lang.clone().or_else(|| vibepod_config.lang());
    let detected_langs: Vec<(String, &'static str)> = if effective_lang.is_none() {
        detect_languages(&cwd)
    } else {
        Vec::new()
    };

    let (mut lang_names, mut lang_display): (Vec<String>, String) =
        if let Some(ref l) = effective_lang {
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

    // Union template-required langs into the install plan. `--lang`
    // / config / auto-detect are respected as the primary source;
    // template required_langs are ADDED on top so a rust-code run in
    // a Python project still installs Rust.
    if !template_metadata.runtime.required_langs.is_empty() {
        let mut template_added: Vec<String> = Vec::new();
        for req in &template_metadata.runtime.required_langs {
            if !lang_names.iter().any(|n| n == req) {
                lang_names.push(req.clone());
                template_added.push(req.clone());
            }
        }
        if !template_added.is_empty() {
            let suffix = format!("+ {} (template)", template_added.join(", "));
            if lang_display.is_empty() {
                lang_display = suffix.trim_start_matches("+ ").to_string();
            } else {
                lang_display = format!("{} {}", lang_display, suffix);
            }
        }
    }

    let setup_cmd: Option<String> = {
        let mut setup_parts: Vec<String> = lang_names
            .iter()
            .filter_map(|l| get_lang_install_cmd(l).map(|s| s.to_string()))
            .collect();
        // Phase 4.7: append template-declared setup_commands AFTER the
        // language install chain. Order matters — commands like
        // `rustup component add rust-analyzer` depend on rustup having
        // already been installed by the `rust` lang step.
        for cmd in &template_metadata.runtime.setup_commands {
            setup_parts.push(cmd.clone());
        }
        if setup_parts.is_empty() {
            None
        } else {
            Some(setup_parts.join(" && "))
        }
    };

    if setup_cmd.is_some() {
        eprintln!("Note: Language/tool setup requires sudo in the container. If setup fails, run `vibepod init` to rebuild the image.");
    }

    // 2. Load global config (config_dir already loaded at the top
    // because template metadata resolution needs it early).
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
    // Container naming:
    //   - worktree: random short hash (disposable)
    //   - template mode: project path + canonical template path → SHA256[:8]
    //     (canonical path keeps macOS case-insensitive FS consistent)
    //   - host mode: project path → SHA256[:8] (v1.4.3 compatible)
    //
    // Template resolution (lazy extract + resolve_template_dir) already
    // happened earlier because the lang pipeline needs the template
    // metadata. Reuse `resolved_template` here instead of re-resolving.
    let container_name = if opts.worktree {
        let short_hash: String = (0..6)
            .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
            .collect();
        format!("vibepod-{}-{}", project_name, short_hash)
    } else if let Some((_, ref canonical_template)) = resolved_template {
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

        // Encode the FULL sorted lang_names set, not just the display's
        // first token. Previously we stored only the first whitespace-
        // delimited word of `lang_display`, which meant e.g. "python
        // (detected from ...) + rust (template)" stored as "python".
        // A pre-existing container (created before the template added
        // rust) would then match the label and be reused WITHOUT
        // running `setup_cmd`, leaving the template-required runtime
        // uninstalled. Storing the whole set forces re-provisioning
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

        current_labels.insert("vibepod.mounts".to_string(), mounts_parts.join("|"));
        current_labels.insert("vibepod.network".to_string(), opts.no_network.to_string());
        current_labels.insert("vibepod.lang".to_string(), current_lang);
        current_labels.insert("vibepod.env_hash".to_string(), current_env_hash);
        // Phase 4.7: include the template_setup_hash so warn_config_changes
        // can surface a drift diff. Note that legacy template containers
        // (labels_version < 3) under a template that declares non-empty
        // setup_commands are NOT silently reused — they are hard-failed
        // by the reuse gate below with a `--new` hint. The presence of
        // this label in current_labels still helps the warning path
        // describe drift in the remaining edge cases (labels_version >= 3
        // containers that hit the gate, host-mode reuse where the value
        // is empty on both sides, etc.).
        current_labels.insert(
            "vibepod.template_setup_hash".to_string(),
            template_setup_hash.clone(),
        );

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

            // Template-required langs must all be present in the stored
            // lang set of the existing container. `setup_cmd` only runs
            // at container creation time, so a container created before
            // the template added a `required_langs` entry will not
            // have that toolchain even though the warning would let
            // the reuse through. Hard-fail here with a clear --new hint.
            //
            // Gate on `vibepod.labels_version >= 2`. Pre-Phase-4.6
            // containers do not have that label; their `vibepod.lang`
            // is in the legacy single-token format that cannot be
            // trusted as the complete installed set (e.g. a polyglot
            // container with multiple langs would still have just the
            // first token stored). Falling through to warn_config_changes
            // in the legacy case gives the user a readable diff instead
            // of forcing --new unnecessarily.
            let labels_version = stored_labels
                .get("vibepod.labels_version")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1);
            if labels_version >= 2 && !template_metadata.runtime.required_langs.is_empty() {
                let stored_lang_set: std::collections::BTreeSet<&str> = stored_labels
                    .get("vibepod.lang")
                    .map(|s| {
                        s.split([',', ' '])
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                let missing: Vec<String> = template_metadata
                    .runtime
                    .required_langs
                    .iter()
                    .filter(|req| !stored_lang_set.contains(req.as_str()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    bail!(
                        "Template '{}' requires language(s) [{}] but the existing \
                         container was created without them (stored lang set: {:?}). \
                         Language installation only runs at container creation, so \
                         the required toolchain cannot be added to the existing \
                         container. Run with --new to recreate the container with \
                         the required toolchain installed.",
                        effective_template.as_deref().unwrap_or(""),
                        missing.join(", "),
                        stored_labels
                            .get("vibepod.lang")
                            .cloned()
                            .unwrap_or_else(|| "(none)".to_string())
                    );
                }
            }

            // Phase 4.7: template_setup_hash drift gate.
            //
            // setup_commands run exactly once, at container creation,
            // chained after the language install. If a template's
            // setup_commands list changes (entry added / removed /
            // reordered / edited), the existing container cannot
            // retroactively re-run the new chain, so reuse would leave
            // the container in a half-configured state relative to the
            // updated template. Hard-fail with a clear --new hint.
            //
            // Two cases:
            //   * labels_version >= 3: compare stored hash to current.
            //     Any difference is drift → bail.
            //   * labels_version < 3 (pre-Phase-4.7 legacy container):
            //     the container has no stored hash at all, so we cannot
            //     tell whether any setup_commands have ever run inside
            //     it. If the current template declares non-empty
            //     setup_commands, we MUST assume the legacy container
            //     is missing them and bail with --new. If the current
            //     template has empty setup_commands there is nothing
            //     to install so reuse is safe.
            let stored_setup_hash = stored_labels
                .get("vibepod.template_setup_hash")
                .cloned()
                .unwrap_or_default();
            let setup_hash_mismatch = if labels_version >= 3 {
                stored_setup_hash != template_setup_hash
            } else {
                // Legacy (no label): treat as "has ever run any
                // setup" = false. Mismatch if the current template
                // wants any setup done.
                !template_metadata.runtime.setup_commands.is_empty()
            };
            if setup_hash_mismatch {
                bail!(
                    "Template '{}' setup_commands hash mismatch (stored: {:?}, \
                     current: {:?}; labels_version={}). setup_commands run only \
                     at container creation, so the declared commands cannot be \
                     retroactively applied to the existing container. Run with \
                     --new to recreate the container with the updated setup \
                     sequence.",
                    effective_template.as_deref().unwrap_or(""),
                    if stored_setup_hash.is_empty() {
                        "(none)".to_string()
                    } else {
                        stored_setup_hash
                    },
                    if template_setup_hash.is_empty() {
                        "(none)".to_string()
                    } else {
                        template_setup_hash.clone()
                    },
                    labels_version
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
        runtime_dir,
        global_config,
        home,
        worktree_branch_name,
        worktree_dir_name,
        lang_display,
        lang_names,
        template_setup_hash,
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
