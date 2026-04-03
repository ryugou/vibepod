use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::runtime::{format_stream_event, ContainerStatus, DockerRuntime, StreamEvent};

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

    let runtime = DockerRuntime::new().await?;

    // コンテナライフサイクルの管理
    match ctx.container_status {
        ContainerStatus::Running => {
            // 実行中でもセットアップ完了マーカーを確認する
            // （初回起動途中で中断された場合にセットアップ未完了のまま残ることがある）
            if !runtime.check_setup_marker(&ctx.container_name).await? {
                // マーカーなし: セットアップ未完了 → コンテナ削除して初回フローへ
                runtime.remove_container(&ctx.container_name).await?;
                create_and_setup(ctx, opts).await?;
            }
        }
        ContainerStatus::Stopped => {
            // 停止中のコンテナを起動してマーカー確認
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
                // マーカーなし: セットアップ未完了 → コンテナ削除して初回フローへ
                runtime.remove_container(&ctx.container_name).await?;
                create_and_setup(ctx, opts).await?;
            }
        }
        ContainerStatus::None => {
            // コンテナなし: 新規作成してセットアップ
            create_and_setup(ctx, opts).await?;
        }
    }

    // セッションを記録
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

    // ログファイルのセットアップ（--prompt モードのみ）
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

    // docker exec -e TOKEN=... {name} bash --login -c 'exec claude "$@"' -- {args}
    // bash --login でログインプロファイルをソースし、setup で設定した PATH を引き継ぐ
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
    exec_args.push("--".to_string()); // $0 placeholder
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

    let (result_text, ctrl_c_pressed): (Option<String>, bool) = tokio::select! {
        r = async {
            let mut rt: Option<String> = None;
            let mut log = log_file;
            while let Ok(Some(line)) = lines.next_line().await {
                // ログファイルに書き出し
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

    // exec の終了ステータスを確認する（Ctrl+C でなければ）
    if ctrl_c_pressed {
        let _ = exec_child.kill().await;
        let _ = exec_child.wait().await;
    } else {
        // プロセスはすでに終了しているので kill 不要
        if let Ok(status) = exec_child.wait().await {
            if let Some(code) = status.code() {
                // 非ゼロ終了かつ結果なしはコンテナ起動失敗の可能性
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

    // コンテナの後処理
    if ctx.is_disposable {
        // 使い捨てコンテナ（--worktree）: 削除
        Command::new("docker")
            .args(["rm", "-f", &ctx.container_name])
            .output()
            .ok();
        // 使い捨てコンテナのみ temp claude.json を削除する
        // 永続コンテナは次回起動時に bind mount が必要なため削除しない
        if let Some(ref temp_cj) = ctx.temp_claude_json {
            std::fs::remove_file(temp_cj).ok();
        }
    } else if ctx.container_status != ContainerStatus::Running {
        // 停止中または新規作成したコンテナ: 停止して保持
        Command::new("docker")
            .args(["stop", "-t", "10", &ctx.container_name])
            .output()
            .ok();
    }
    // 元から実行中だったコンテナは停止しない

    print_post_run_summary(opts, ctx, result_text.as_deref(), ctx.is_disposable);
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
