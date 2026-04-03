use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::config::{self, ProjectEntry};
use crate::git;
use crate::runtime::DockerRuntime;
use crate::session::{self, SessionStore};
use crate::ui::{banner, prompts};

use super::{
    build_review_prompt, detect_languages, get_lang_install_cmd, parse_mount_arg,
    resolve_reviewers, RunContext, RunOptions, VALID_REVIEWERS,
};

pub(super) async fn prepare_context(opts: &RunOptions) -> Result<Option<RunContext>> {
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

    // --reuse: use a fixed container name and detect stopped containers for reconnection.
    // Running reuse containers are handled below by the normal running-container prompt.
    let (reuse_existing, container_name_override) = if opts.reuse {
        let reuse_name = format!("vibepod-{}-reuse", project_name);
        let existing_id = runtime.find_stopped_container(&reuse_name).await?;
        (existing_id.is_some(), Some(reuse_name))
    } else {
        (false, None)
    };

    // If a container with the project prefix is already running (and we're not reusing it),
    // prompt the user to attach or replace it.
    if !reuse_existing {
        if let Some((existing_id, existing_name)) =
            runtime.find_running_container(&name_prefix).await?
        {
            if !interactive {
                // Non-interactive mode: skip prompt, just replace
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
            } else {
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
            } // else (interactive)
        }
    } // if !reuse_existing

    // 5. Project registration
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
    let container_name = if let Some(override_name) = container_name_override {
        override_name
    } else {
        let short_hash: String = (0..6)
            .map(|_| format!("{:x}", rand::random::<u8>() & 0x0f))
            .collect();
        format!("vibepod-{}-{}", project_name, short_hash)
    };

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
        reuse: opts.reuse,
        reuse_existing,
    }))
}
