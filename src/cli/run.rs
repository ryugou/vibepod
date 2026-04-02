use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::config::{self, ProjectEntry};
use crate::git;
use crate::runtime::{format_stream_event, ContainerConfig, DockerRuntime, StreamEvent};
use crate::session::{self, SessionStore};
use crate::ui::banner;
use crate::ui::prompts;

pub struct RunOptions {
    pub resume: bool,
    pub prompt: Option<String>,
    pub no_network: bool,
    pub env_vars: Vec<String>,
    pub env_file: Option<String>,
    pub lang: Option<String>,
    pub worktree: bool,
    pub review: Option<String>,
    pub mount: Vec<String>,
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

struct RunContext {
    container_name: String,
    effective_workspace: String,
    claude_args: Vec<String>,
    resolved_env_vars: Vec<String>,
    setup_cmd: Option<String>,
    temp_claude_json: Option<std::path::PathBuf>,
    global_config: config::GlobalConfig,
    home: std::path::PathBuf,
    worktree_branch_name: Option<String>,
    worktree_dir_name: Option<String>,
    lang_display: String,
    reviewers: Vec<String>,
    codex_auth: Option<String>,
    store: SessionStore,
    deferred_session: session::Session,
    extra_mounts: Vec<(String, String)>,
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
        "go" => Some("ARCH=$(uname -m) && GOARCH=$([ \"$ARCH\" = \"aarch64\" ] && echo arm64 || echo amd64) && curl -fsSL https://go.dev/dl/go1.24.2.linux-${GOARCH}.tar.gz | sudo tar -C /usr/local -xzf - && export PATH=$PATH:/usr/local/go/bin"),
        "java" => Some("sudo apt-get update && sudo apt-get install -y default-jdk"),
        _ => None,
    }
}

pub fn validate_slack_channel_id(id: &str) -> bool {
    (id.starts_with('C') || id.starts_with('G')) && id.len() >= 9
}

const VALID_REVIEWERS: &[&str] = &["copilot", "codex"];

pub fn resolve_reviewers(review_arg: &Option<String>, config: &[String]) -> Vec<String> {
    match review_arg {
        None => vec![],
        Some(explicit) if !explicit.is_empty() => {
            if VALID_REVIEWERS.contains(&explicit.as_str()) {
                vec![explicit.clone()]
            } else {
                vec![]
            }
        }
        Some(_) => config
            .iter()
            .filter(|r| VALID_REVIEWERS.contains(&r.as_str()))
            .cloned()
            .collect(),
    }
}

pub fn build_review_prompt(prompt: &str, reviewers: &[String]) -> String {
    if reviewers.is_empty() {
        return prompt.to_string();
    }

    let has_codex = reviewers.contains(&"codex".to_string());
    let has_copilot = reviewers.contains(&"copilot".to_string());

    if !has_codex && !has_copilot {
        return prompt.to_string();
    }

    let mut sections: Vec<String> = Vec::new();

    sections.push(
        "## 共通準備\n\
- 現在のブランチが main の場合は `git checkout -b <適切なブランチ名>` で新しいブランチを作成する"
            .to_string(),
    );

    // Codex review フェーズ（ローカル、コミット前）
    if has_codex {
        sections.push(
            "## Codex Review（ローカル、コミット前）\n\
以下を指摘がなくなるまで繰り返す（最大 5 回）:\n\
1. Bash ツールで `codex review -c sandbox_mode=danger-full-access -c approval_policy=never` を実行する（timeout: 600000 を必ず指定すること。デフォルトの 120 秒ではタイムアウトする）\n\
   （重要: Claude Code の内蔵レビュー機能やスキルではなく、Codex CLI コマンドを Bash で直接実行すること。コンテナ内では Linux namespace が使えないため sandbox を無効化し、非対話実行のため approval も無効化する）\n\
2. 出力を確認する。「指摘なし」「no issues」等であればこのフェーズ完了\n\
3. 指摘があれば該当箇所を修正する\n\
4. 手順 1 に戻る"
                .to_string(),
        );
    }

    // コミット + push + PR 作成
    sections.push(
        "## コミットと PR 作成\n\
1. 変更内容をコミットする（Conventional Commits 準拠）\n\
2. `git push -u origin <ブランチ名>` でリモートに push する\n\
3. `gh pr create --base main` で PR を作成する"
            .to_string(),
    );

    // Copilot review フェーズ（PR 上、1ラウンドのみ。API での re-review は未サポート）
    if has_copilot {
        sections.push(
            "## Copilot Review（PR 上、1ラウンド）\n\
1. `gh pr edit <PR番号> --add-reviewer copilot` で Copilot レビューを依頼する\n\
2. 30 秒間隔で最大 10 回 `gh api repos/{owner}/{repo}/pulls/{number}/reviews` をポーリングする\n\
   （重要: `gh pr review` や `gh pr comment` 等の書き込み系コマンドは絶対に使わないこと）\n\
3. レビュー結果を確認する。インラインコメントは `gh api repos/{owner}/{repo}/pulls/{number}/comments` で取得する\n\
4. 指摘があれば修正し、コミットして `git push` する\n\
注意: Copilot の re-review は API から自動でリクエストできないため、1ラウンドで終了する"
                .to_string(),
        );
    }

    sections.push("## 完了\n- 最終的な PR の URL を出力する".to_string());

    format!(
        "{}\n\n---\n\n【必須】上記の作業が終わったら、以下のレビューフローを必ず最後まで実行すること。レビューフローを省略してはならない。\n\n{}",
        prompt,
        sections.join("\n\n")
    )
}

fn build_container_config(ctx: &RunContext, image: String, no_network: bool) -> ContainerConfig {
    let gitconfig = ctx.home.join(".gitconfig");
    ContainerConfig {
        image,
        container_name: ctx.container_name.clone(),
        workspace_path: ctx.effective_workspace.clone(),
        claude_json: ctx
            .temp_claude_json
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        args: {
            let mut full = vec!["claude".to_string()];
            full.extend(ctx.claude_args.clone());
            full
        },
        gitconfig: if gitconfig.exists() {
            Some(gitconfig.to_string_lossy().to_string())
        } else {
            None
        },
        env_vars: ctx.resolved_env_vars.clone(),
        network_disabled: no_network,
        setup_cmd: ctx.setup_cmd.clone(),
        codex_auth: ctx.codex_auth.clone(),
        extra_mounts: ctx.extra_mounts.clone(),
    }
}

async fn prepare_context(opts: &RunOptions) -> Result<Option<RunContext>> {
    let interactive = !opts.resume && opts.prompt.is_none();

    // 1. Check git repo
    let cwd = std::env::current_dir()?;
    if !git::is_git_repo(&cwd) {
        bail!("Not a git repository. Run this command inside a git-initialized directory.");
    }
    let cwd_str = cwd.to_string_lossy().to_string();

    if opts.worktree && opts.prompt.is_none() {
        bail!("--worktree requires --prompt");
    }
    if opts.review.is_some() && opts.prompt.is_none() {
        bail!("--review requires --prompt");
    }

    // Record session for restore
    let head_before = git::get_head_hash(&cwd)?;
    let current_branch = git::get_current_branch(&cwd).unwrap_or_else(|_| "unknown".to_string());

    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir.clone());

    // Ensure .worktrees/ exists for superpowers skill (must find it without asking the user)
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
    // We prepare the data here but call store.add() later in each run_* function.
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

    let project_name = cwd
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

    // Resolve reviewers (before setup_cmd so codex can extend it)
    let config_reviewers = vibepod_config.reviewers();
    let review_explicit = opts
        .review
        .as_deref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if review_explicit {
        if let Some(ref v) = opts.review {
            if !VALID_REVIEWERS.contains(&v.as_str()) {
                bail!(
                    "Unknown reviewer '{}'. Valid values: {}",
                    v,
                    VALID_REVIEWERS.join(", ")
                );
            }
        }
    }
    let mut reviewers = resolve_reviewers(&opts.review, &config_reviewers);

    // Codex CLI availability check
    if reviewers.contains(&"codex".to_string()) {
        let codex_available = Command::new("which")
            .arg("codex")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !codex_available {
            if review_explicit {
                bail!("Codex CLI is not installed. Install it with: npm install -g @openai/codex");
            } else {
                eprintln!("Warning: Codex CLI not found, skipping codex review");
                reviewers.retain(|r| r != "codex");
            }
        }
    }

    // Codex auth mount path
    let home_early = config::home_dir()?;
    let codex_auth = if reviewers.contains(&"codex".to_string()) {
        let codex_auth_path = home_early.join(".codex/auth.json");
        if codex_auth_path.exists() {
            Some(codex_auth_path.to_string_lossy().to_string())
        } else {
            None
        }
    } else {
        None
    };

    let setup_cmd: Option<String> = {
        let mut setup_parts: Vec<String> = lang_names
            .iter()
            .filter_map(|l| get_lang_install_cmd(l).map(|s| s.to_string()))
            .collect();
        if reviewers.contains(&"codex".to_string()) {
            let setup_cmd_contains_node = lang_names.contains(&"node".to_string());
            if !setup_cmd_contains_node {
                setup_parts.push("curl -fsSL https://deb.nodesource.com/setup_22.x | sudo bash - && sudo apt-get install -y nodejs".to_string());
            }
            setup_parts.push("sudo npm install -g @openai/codex".to_string());
            // Fix ownership of ~/.codex/ dir (Docker creates it as root when bind-mounting auth.json)
            // Only chown the directory itself, not -R (auth.json is read-only bind-mount)
            setup_parts.push(
                "(if [ -d \"$HOME/.codex\" ]; then sudo chown $(id -u):$(id -g) \"$HOME/.codex\" 2>/dev/null || true; fi)".to_string(),
            );
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

    // 2. Load config
    let config_dir = config::default_config_dir()?;
    let global_config = config::load_global_config(&config_dir)?;

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

    // 4. Check for existing container
    let name_prefix = format!("vibepod-{}", project_name);
    if let Some((existing_id, existing_name)) = runtime.find_running_container(&name_prefix).await?
    {
        match prompts::handle_existing_container(&existing_name)? {
            prompts::ExistingContainerAction::Attach => {
                println!("  ◇  Attaching to {}...", existing_name);
                runtime.stream_logs(&existing_id).await?;
                return Ok(None);
            }
            prompts::ExistingContainerAction::Replace => {
                let stop = Command::new("docker")
                    .args(["stop", "-t", "10", &existing_id])
                    .output()
                    .context("Failed to run docker stop")?;
                if !stop.status.success() {
                    bail!(
                        "Failed to stop container {}: {}",
                        existing_name,
                        String::from_utf8_lossy(&stop.stderr).trim()
                    );
                }
                let rm = Command::new("docker")
                    .args(["rm", "-f", &existing_id])
                    .output()
                    .context("Failed to run docker rm")?;
                if !rm.status.success() {
                    bail!(
                        "Failed to remove container {}: {}",
                        existing_name,
                        String::from_utf8_lossy(&rm.stderr).trim()
                    );
                }
            }
        }
    }

    // 5. Project registration
    let mut projects = config::load_projects(&config_dir)?;
    if !config::is_project_registered(&projects, &cwd_str)
        && prompts::confirm_project_registration(&project_name)?
    {
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

    // 6. Build claude args
    let mut claude_args: Vec<String> = Vec::new();
    if !interactive {
        claude_args.push("--dangerously-skip-permissions".to_string());
    }
    if opts.resume {
        claude_args.push("--resume".to_string());
    }
    if let Some(ref p) = opts.prompt {
        let effective_prompt = build_review_prompt(p, &reviewers);
        claude_args.push("-p".to_string());
        claude_args.push(effective_prompt);
        claude_args.push("--output-format".to_string());
        claude_args.push("stream-json".to_string());
        claude_args.push("--verbose".to_string());
    }

    // 7. Resolve env file if provided
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

    // 8. Generate container name
    let short_hash: String = (0..6)
        .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
        .collect();
    let container_name = format!("vibepod-{}-{}", project_name, short_hash);

    // 9. Auth: load token
    let auth_manager = crate::auth::AuthManager::new(config_dir.clone());
    let home = crate::config::home_dir()?;
    let claude_json = home.join(".claude.json");

    let token_data = auth_manager
        .load_token()?
        .context("Not authenticated. Run `vibepod login` first.")?;

    if token_data.needs_renewal() {
        bail!("Token expires soon. Please run `vibepod login` to renew.");
    }

    // Add token as environment variable
    resolved_env_vars.push(format!("CLAUDE_CODE_OAUTH_TOKEN={}", token_data.token));

    // GitHub token: gh auth token でホスト側のトークンを自動取得
    if let Ok(output) = Command::new("gh").args(["auth", "token"]).output() {
        if output.status.success() {
            let gh_token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !gh_token.is_empty() {
                resolved_env_vars.push(format!("GH_TOKEN={}", gh_token));
            }
        }
    }

    // Copy .claude.json to temp file to protect host file from container writes
    let temp_claude_json = if claude_json.exists() {
        let temp_dir = std::env::temp_dir().join("vibepod-run");
        std::fs::create_dir_all(&temp_dir)?;
        let temp_path = temp_dir.join("claude.json");
        std::fs::copy(&claude_json, &temp_path)?;
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

    Ok(Some(RunContext {
        container_name,
        effective_workspace,
        claude_args,
        resolved_env_vars,
        setup_cmd,
        temp_claude_json,
        global_config,
        home,
        worktree_branch_name,
        worktree_dir_name,
        lang_display,
        reviewers,
        codex_auth,
        store,
        deferred_session,
        extra_mounts,
    }))
}

async fn run_interactive(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
    println!("  ◇  Starting container...");
    println!("  │  Agent: Claude Code");
    println!("  │  Mode: interactive");
    println!("  │  Mount: {} → /workspace", ctx.effective_workspace);
    for (host, container) in &ctx.extra_mounts {
        println!("  │  Mount (ro): {} → {}", host, container);
    }
    if !ctx.lang_display.is_empty() {
        println!("  │  Language: {}", ctx.lang_display);
    }
    println!("  │");

    let container_config =
        build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
    let docker_args = container_config.to_docker_args(true);

    // Record session now that container is about to start
    ctx.store.add(ctx.deferred_session.clone())?;

    println!("  ◇  Container: {}", ctx.container_name);
    println!("  └\n");

    let status = Command::new("docker")
        .args(&docker_args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to run container")?;

    if !status.success() {
        // Claude exited with non-zero, which is fine (e.g., user quit)
    }

    // Clean up temp claude.json
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    println!("  Container stopped and removed.");
    Ok(())
}

async fn run_fire_and_forget(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
    let mode_label = if opts.resume {
        "resume (--dangerously-skip-permissions)"
    } else {
        "fire-and-forget (--dangerously-skip-permissions)"
    };
    if opts.prompt.is_some() {
        println!("Starting container...");
        println!("Agent: Claude Code");
        println!("Mode: {}", mode_label);
        println!("Mount: {} → /workspace", ctx.effective_workspace);
        for (host, container) in &ctx.extra_mounts {
            println!("Mount (ro): {} → {}", host, container);
        }
        if !ctx.lang_display.is_empty() {
            println!("Language: {}", ctx.lang_display);
        }
        if !ctx.reviewers.is_empty() {
            println!("Review: enabled ({})", ctx.reviewers.join(", "));
        }
        println!();
    } else {
        println!("  ◇  Starting container...");
        println!("  │  Agent: Claude Code");
        println!("  │  Mode: {}", mode_label);
        println!("  │  Mount: {} → /workspace", ctx.effective_workspace);
        for (host, container) in &ctx.extra_mounts {
            println!("  │  Mount (ro): {} → {}", host, container);
        }
        if !ctx.lang_display.is_empty() {
            println!("  │  Language: {}", ctx.lang_display);
        }
        println!("  │");
    }

    let container_config =
        build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
    let docker_run_args = container_config.to_docker_args(false);

    // Start container with docker run -d
    let start_output = Command::new("docker")
        .args(&docker_run_args)
        .output()
        .context("Failed to start container")?;

    if !start_output.status.success() {
        bail!(
            "Failed to start container: {}",
            String::from_utf8_lossy(&start_output.stderr).trim()
        );
    }

    // Record session now that container has actually started
    ctx.store.add(ctx.deferred_session.clone())?;

    if opts.prompt.is_some() {
        println!("Container started: {}", ctx.container_name);
        println!("Press Ctrl+C to stop the container.");
        println!();
    } else {
        println!("  ◇  Container started: {}", ctx.container_name);
        println!("  │  Press Ctrl+C to stop the container.");
        println!("  └\n");
    }

    let separator = "────────────────────────────────────────────────────────";
    if opts.prompt.is_some() {
        println!("{}", separator);
    }

    // Stream logs with docker logs --follow
    let mut log_child = tokio::process::Command::new("docker")
        .args(["logs", "--follow", &ctx.container_name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .context("Failed to run docker logs")?;

    let stdout = log_child
        .stdout
        .take()
        .context("Failed to capture docker logs stdout")?;

    let is_prompt = opts.prompt.is_some();
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = tokio::io::AsyncBufReadExt::lines(reader);

    let result_text: Option<String> = tokio::select! {
        r = async {
            let mut rt: Option<String> = None;
            while let Ok(Some(line)) = lines.next_line().await {
                if is_prompt {
                    match format_stream_event(&line) {
                        StreamEvent::Display(s) => println!("{}", s),
                        StreamEvent::Result(s) => rt = Some(s),
                        StreamEvent::Skip => {}
                        StreamEvent::PassThrough(s) => println!("{}", s),
                    }
                } else {
                    println!("{}", line);
                }
            }
            rt
        } => r,
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping container...");
            None
        }
    };

    let _ = log_child.kill().await;

    // docker stop + docker rm -f
    Command::new("docker")
        .args(["stop", "-t", "10", &ctx.container_name])
        .output()
        .ok();
    Command::new("docker")
        .args(["rm", "-f", &ctx.container_name])
        .output()
        .ok();

    if opts.prompt.is_some() {
        println!("{}", separator);
    }

    // Clean up temp claude.json
    if let Some(ref temp_cj) = ctx.temp_claude_json {
        std::fs::remove_file(temp_cj).ok();
    }

    if opts.prompt.is_some() {
        if let Some(ref text) = result_text {
            println!();
            println!("Result:");
            println!("{}", text);
        }

        // diff summary and worktree info
        let diff_dir = std::path::Path::new(&ctx.effective_workspace);
        let output = Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(diff_dir)
            .output()?;
        let stat = String::from_utf8_lossy(&output.stdout);
        if !stat.trim().is_empty() {
            println!();
            println!("Changes:");
            for line in stat.lines() {
                println!("{}", line);
            }
        }

        if let (Some(ref branch), Some(ref dir)) =
            (&ctx.worktree_branch_name, &ctx.worktree_dir_name)
        {
            println!();
            println!("Worktree: .worktrees/{}", dir);
            println!("Branch: {}", branch);
            println!("To review: cd .worktrees/{} && git diff main", dir);
            println!("To merge:  git merge {}", branch);
            println!("To remove: git worktree remove .worktrees/{}", dir);
        }

        println!();
        println!("Container stopped and removed.");
    } else {
        if let (Some(ref branch), Some(ref dir)) =
            (&ctx.worktree_branch_name, &ctx.worktree_dir_name)
        {
            println!("  ◇  Worktree: .worktrees/{}", dir);
            println!("  │  Branch: {}", branch);
            println!("  │  To review: cd .worktrees/{} && git diff main", dir);
            println!("  │  To merge:  git merge {}", branch);
            println!("  │  To remove: git worktree remove .worktrees/{}", dir);
        }

        println!("  Container stopped and removed.");
    }

    Ok(())
}

pub async fn execute(opts: RunOptions) -> Result<()> {
    let interactive = !opts.resume && opts.prompt.is_none();

    let Some(ctx) = prepare_context(&opts).await? else {
        return Ok(());
    };

    if interactive {
        run_interactive(&opts, &ctx).await
    } else {
        run_fire_and_forget(&opts, &ctx).await
    }
}
