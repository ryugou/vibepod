use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::runtime::{format_stream_event, ContainerStatus, DockerRuntime, StreamEvent};
use libc;

use super::{build_container_config, RunContext, RunOptions};

/// コンテナを作成してセットアップを実行する（初回フロー）。
/// セットアップ失敗時はコンテナを自動削除してエラーを返す。
async fn create_and_setup(ctx: &RunContext, opts: &RunOptions) -> Result<()> {
    let container_config =
        build_container_config(ctx, ctx.global_config.image.clone(), opts.no_network);
    let create_args = container_config.to_create_args();

    let output = Command::new("docker")
        .args(&create_args)
        .output()
        .context("Failed to create container")?;

    if !output.status.success() {
        bail!(
            "Failed to create container: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    // セットアップコマンドを実行してマーカーを作成する
    let setup_result = if let Some(ref setup_cmd) = ctx.setup_cmd {
        let full_cmd = format!("{} && touch /home/vibepod/.vibepod-setup-done", setup_cmd);
        Command::new("docker")
            .args(["exec", &ctx.container_name, "sh", "-c", &full_cmd])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run setup command")?
    } else {
        Command::new("docker")
            .args([
                "exec",
                &ctx.container_name,
                "touch",
                "/home/vibepod/.vibepod-setup-done",
            ])
            .status()
            .context("Failed to create setup marker")?
    };

    if !setup_result.success() {
        // セットアップ失敗: コンテナを自動削除
        Command::new("docker")
            .args(["rm", "-f", &ctx.container_name])
            .output()
            .ok();
        bail!(
            "Container setup failed. Container has been removed. \
             Check the output above for errors."
        );
    }

    Ok(())
}

pub(super) async fn run_fire_and_forget(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
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

    // ロックを取得（コンテナ起動・セッション記録より前に取得し、
    // 同時起動時に片方がコンテナ状態を変更してしまうのを防ぐ）
    let vibepod_dir = std::path::PathBuf::from(&ctx.effective_workspace).join(".vibepod");
    let prompt_text = opts
        .prompt
        .as_deref()
        .unwrap_or("--resume")
        .chars()
        .take(200)
        .collect::<String>();
    let lock = super::lock::PromptLock::acquire(vibepod_dir, prompt_text)?;

    let runtime = DockerRuntime::new().await?;

    match ctx.container_status {
        ContainerStatus::Running => {
            if !runtime.check_setup_marker(&ctx.container_name).await? {
                runtime.remove_container(&ctx.container_name).await?;
                create_and_setup(ctx, opts).await?;
            }
        }
        ContainerStatus::Stopped => {
            let start = Command::new("docker")
                .args(["start", &ctx.container_name])
                .output()
                .context("Failed to start container")?;
            if !start.status.success() {
                bail!(
                    "Failed to start container: {}",
                    String::from_utf8_lossy(&start.stderr).trim()
                );
            }
            if !runtime.check_setup_marker(&ctx.container_name).await? {
                runtime.remove_container(&ctx.container_name).await?;
                create_and_setup(ctx, opts).await?;
            }
        }
        ContainerStatus::None => {
            create_and_setup(ctx, opts).await?;
        }
    }

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

    let log_file = if opts.prompt.is_some() {
        let session_dir = std::path::Path::new(&ctx.effective_workspace)
            .join(".vibepod")
            .join("sessions")
            .join(&ctx.deferred_session.id);
        std::fs::create_dir_all(&session_dir)?;
        let log_path = session_dir.join("logs.txt");
        Some(std::fs::File::create(&log_path).context("Failed to create log file")?)
    } else {
        None
    };

    let mut exec_args = vec!["exec".to_string()];
    for env_var in &ctx.exec_env_vars {
        exec_args.push("-e".to_string());
        exec_args.push(env_var.clone());
    }
    exec_args.push(ctx.container_name.clone());
    exec_args.push("bash".to_string());
    exec_args.push("--login".to_string());
    exec_args.push("-c".to_string());
    exec_args.push(r#"exec claude "$@""#.to_string());
    exec_args.push("--".to_string());
    exec_args.extend(ctx.claude_args.clone());

    let mut exec_child = tokio::process::Command::new("docker")
        .args(&exec_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .context("Failed to exec claude in container")?;

    let stdout = exec_child
        .stdout
        .take()
        .context("Failed to capture exec stdout")?;

    let is_prompt = opts.prompt.is_some();
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = tokio::io::AsyncBufReadExt::lines(reader);

    // ストリーム途絶監視用の共有状態
    let last_event_at = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let idle_timeout_secs = ctx.prompt_idle_timeout;
    let timed_out = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // 監視タスク（idle_timeout > 0 の場合のみ）
    let monitor_handle = if idle_timeout_secs > 0 {
        let last_event = last_event_at.clone();
        let timed_out_flag = timed_out.clone();
        let child_id = exec_child.id();
        Some(tokio::spawn(async move {
            let timeout = std::time::Duration::from_secs(idle_timeout_secs);
            let check_interval = std::time::Duration::from_secs(idle_timeout_secs.min(30));
            loop {
                tokio::time::sleep(check_interval).await;
                let elapsed = last_event.lock().unwrap().elapsed();
                if elapsed > timeout {
                    timed_out_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    if let Some(pid) = child_id {
                        unsafe {
                            libc::kill(pid as i32, libc::SIGTERM);
                        }
                    }
                    break;
                }
            }
        }))
    } else {
        None
    };

    let (result_text, ctrl_c_pressed): (Option<String>, bool) = tokio::select! {
        r = async {
            let mut rt: Option<String> = None;
            let mut log = log_file;
            let mut event_count: u64 = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                *last_event_at.lock().unwrap() = std::time::Instant::now();

                event_count += 1;
                if event_count.is_multiple_of(30) {
                    lock.update_last_event().ok();
                }

                if let Some(ref mut f) = log {
                    use std::io::Write;
                    let _ = writeln!(f, "{}", line);
                }
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
            (rt, false)
        } => r,
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping container...");
            (None, true)
        }
    };

    if let Some(handle) = monitor_handle {
        handle.abort();
    }

    let was_timed_out = timed_out.load(std::sync::atomic::Ordering::SeqCst);

    if ctrl_c_pressed || was_timed_out {
        let _ = exec_child.kill().await;
        let _ = exec_child.wait().await;
    } else {
        if let Ok(status) = exec_child.wait().await {
            if let Some(code) = status.code() {
                if code != 0 && result_text.is_none() {
                    eprintln!(
                        "Warning: docker exec exited with code {} (container may have failed to \
                         start Claude). Use `vibepod logs {}` to inspect.",
                        code, ctx.container_name
                    );
                }
            }
        }
    }

    if opts.prompt.is_some() {
        println!("{}", separator);
    }

    // タイムアウト時の自動リセット
    if was_timed_out {
        let workspace_path = std::path::Path::new(&ctx.effective_workspace);
        // uncommitted changes だけでなく、コミットが進んでいるかも確認
        // （エージェントが commit した後にタイムアウトするケース）
        let current_head = crate::git::get_head_hash(workspace_path).unwrap_or_default();
        let needs_reset = current_head != ctx.deferred_session.head_before
            || crate::git::has_uncommitted_changes(workspace_path);

        if needs_reset {
            crate::git::reset_hard(workspace_path, &ctx.deferred_session.head_before)?;
            crate::git::clean_fd(workspace_path)?;
        }
        ctx.store.mark_restored(&ctx.deferred_session.id)?;

        let timeout_display = if idle_timeout_secs >= 60 {
            format!("{} 分", idle_timeout_secs / 60)
        } else {
            format!("{} 秒", idle_timeout_secs)
        };
        eprintln!();
        eprintln!(
            "⚠ ストリーム無出力が {} を超えたため、セッションを中断しました。",
            timeout_display
        );
        if needs_reset {
            eprintln!(
                "  作業ディレクトリを {} にリセットしました。",
                &ctx.deferred_session.head_before[..8.min(ctx.deferred_session.head_before.len())]
            );
        }
    }

    // コンテナの後処理
    if ctx.is_disposable {
        Command::new("docker")
            .args(["rm", "-f", &ctx.container_name])
            .output()
            .ok();
        if let Some(ref temp_cj) = ctx.temp_claude_json {
            std::fs::remove_file(temp_cj).ok();
        }
    } else if ctx.container_status != ContainerStatus::Running {
        Command::new("docker")
            .args(["stop", "-t", "10", &ctx.container_name])
            .output()
            .ok();
    }

    // ロック解放（コンテナ後処理が完了してから解放し、
    // 次の起動がコンテナ停止中に走る競合を防ぐ）
    drop(lock);

    if !was_timed_out {
        print_post_run_summary(opts, ctx, result_text.as_deref(), ctx.is_disposable);
    }

    Ok(())
}

fn print_post_run_summary(
    opts: &RunOptions,
    ctx: &RunContext,
    result_text: Option<&str>,
    disposable: bool,
) {
    let stopped_msg = if disposable {
        "Container stopped and removed."
    } else if ctx.container_status == ContainerStatus::Running {
        "Disconnected from container (still running)."
    } else {
        "Container stopped (container preserved for next run)."
    };

    if opts.prompt.is_some() {
        if let Some(text) = result_text {
            println!();
            println!("Result:");
            println!("{}", text);
        }

        // diff summary and worktree info
        let diff_dir = std::path::Path::new(&ctx.effective_workspace);
        if let Ok(output) = Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(diff_dir)
            .output()
        {
            let stat = String::from_utf8_lossy(&output.stdout);
            if !stat.trim().is_empty() {
                println!();
                println!("Changes:");
                for line in stat.lines() {
                    println!("{}", line);
                }
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
        println!("{}", stopped_msg);
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

        println!("  {}", stopped_msg);
    }
}
