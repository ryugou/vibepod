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
    let mode_label = if opts.mode == crate::cli::RunMode::Review {
        if opts.resume {
            "resume (review, permissions enforced)"
        } else {
            "fire-and-forget (review, permissions enforced)"
        }
    } else if opts.resume {
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

    // ロックと idle 監視は --prompt 時のみ有効。
    // --resume は stream-json 出力ではないため JSONL 途絶検知の対象外。
    let is_prompt_mode = opts.prompt.is_some();
    let vibepod_dir = std::path::PathBuf::from(&ctx.effective_workspace).join(".vibepod");
    let lock = if is_prompt_mode {
        let prompt_text = opts
            .prompt
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect::<String>();
        Some(super::lock::PromptLock::acquire(vibepod_dir, prompt_text)?)
    } else {
        None
    };

    let runtime = DockerRuntime::new().await?;

    // 新規作成されたコンテナかどうか（更新チェックの throttle 判定に使う）。
    // 詳細は interactive.rs の同名変数のコメントを参照。
    let container_created = match ctx.container_status {
        ContainerStatus::Running => {
            if !runtime.check_setup_marker(&ctx.container_name).await? {
                runtime.remove_container(&ctx.container_name).await?;
                create_and_setup(ctx, opts).await?;
                true
            } else {
                false
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
                true
            } else {
                false
            }
        }
        ContainerStatus::None => {
            create_and_setup(ctx, opts).await?;
            true
        }
    };

    // コンテナ内 Claude Code の更新（失敗しても続行する。詳細は update モジュール）。
    // Claude の exec 前に済ませることで、この run から新しいバージョンが使われる。
    crate::update::maybe_update_claude(
        &ctx.config_dir,
        &ctx.container_name,
        opts.update_policy,
        opts.no_network,
        container_created,
    )
    .await;

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

    // logs.txt は要約・タイムアウトメッセージでもパスを提示するため、
    // File と一緒にパスも保持する。生 stream-json の保存は verbose 有無に
    // かかわらず常に継続する（要約は表示層の変更で、記録は減らさない）。
    let (log_file, log_path) = if opts.prompt.is_some() {
        let session_dir = std::path::Path::new(&ctx.effective_workspace)
            .join(".vibepod")
            .join("sessions")
            .join(&ctx.deferred_session.id);
        std::fs::create_dir_all(&session_dir)?;
        let log_path = session_dir.join("logs.txt");
        let file = std::fs::File::create(&log_path).context("Failed to create log file")?;
        (Some(file), Some(log_path))
    } else {
        (None, None)
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

    // タイムアウトリセット用: セッション開始前のダーティ状態を記録
    let was_dirty_before =
        crate::git::has_uncommitted_changes(std::path::Path::new(&ctx.effective_workspace));

    // ストリーム途絶監視用の共有状態
    let last_event_at = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let idle_timeout_secs = ctx.prompt_idle_timeout;
    let timed_out = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // 監視タスク（--prompt かつ idle_timeout > 0 の場合のみ）
    let container_name_for_monitor = ctx.container_name.clone();
    let monitor_handle = if is_prompt_mode && idle_timeout_secs > 0 {
        let last_event = last_event_at.clone();
        let timed_out_flag = timed_out.clone();
        let child_id = exec_child.id();
        Some(tokio::spawn(async move {
            let timeout = std::time::Duration::from_secs(idle_timeout_secs);
            let check_interval = std::time::Duration::from_secs(idle_timeout_secs.min(30));
            loop {
                tokio::time::sleep(check_interval).await;
                // unwrap 許容: この Mutex<Instant> は monitor タスクと本体の
                // ストリームループの 2 者だけが、`Instant` の read/write という
                // パニックし得ない極小クリティカルセクションで保持する。
                // 保持中にパニックする経路が無いため poison しない（規約: unwrap
                // にはパニック不可の理由を明記する）。
                let elapsed = last_event.lock().unwrap().elapsed();
                if elapsed > timeout {
                    timed_out_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    // ローカルの docker exec プロセスを終了
                    if let Some(pid) = child_id {
                        unsafe {
                            libc::kill(pid as i32, libc::SIGTERM);
                        }
                    }
                    // コンテナ内の claude プロセスも停止（ワークスペースへの書き込みを止める）
                    // -x: 完全一致（/.claude/ パス等を誤 kill しない）
                    tokio::process::Command::new("docker")
                        .args(["exec", &container_name_for_monitor, "pkill", "-x", "claude"])
                        .output()
                        .await
                        .ok();
                    break;
                }
            }
        }))
    } else {
        None
    };

    // `--verbose` のとき per-event の整形ログを stdout に流す。既定 false
    // では per-event を出さず、末尾で要約のみを出す（生ログは logs.txt に
    // 常に保存される）。--resume は stream-json ではないため従来通り生行を
    // そのまま流す。
    let verbose = ctx.verbose;
    let overall_timeout_secs = ctx.overall_timeout;

    // select! の結果を明示的に区別するためのローカル列挙。
    enum Outcome {
        // (result 本文, 生の result イベント行)
        Completed(Option<String>, Option<String>),
        CtrlC,
        OverallTimeout,
    }

    let outcome = tokio::select! {
        res = async {
            let mut rt: Option<String> = None;
            let mut result_line: Option<String> = None;
            let mut log = log_file;
            let mut last_lock_update = std::time::Instant::now();
            while let Ok(Some(line)) = lines.next_line().await {
                // unwrap 許容: 上の monitor タスクと同じ Mutex<Instant>。保持は
                // `Instant` の代入のみでパニック経路が無く poison しないため
                // （規約: unwrap にはパニック不可の理由を明記する）。
                *last_event_at.lock().unwrap() = std::time::Instant::now();

                // ロックファイルの last_event_at を約 30 秒ごとに更新（vibepod ps 用）
                if last_lock_update.elapsed().as_secs() >= 30 {
                    if let Some(ref l) = lock {
                        l.update_last_event().ok();
                    }
                    last_lock_update = std::time::Instant::now();
                }

                if let Some(ref mut f) = log {
                    use std::io::Write;
                    let _ = writeln!(f, "{}", line);
                }
                if is_prompt {
                    match format_stream_event(&line) {
                        StreamEvent::Display(s) => {
                            if verbose {
                                println!("{}", s);
                            }
                        }
                        StreamEvent::Result(s) => {
                            rt = Some(s);
                            // 生の result イベント行を保持して末尾の要約で
                            // 成否・subtype を判定する。
                            result_line = Some(line.clone());
                        }
                        StreamEvent::Skip => {}
                        StreamEvent::PassThrough(s) => {
                            if verbose {
                                println!("{}", s);
                            }
                        }
                    }
                } else {
                    println!("{}", line);
                }
            }
            (rt, result_line)
        } => {
            let (rt, rl) = res;
            Outcome::Completed(rt, rl)
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping container...");
            Outcome::CtrlC
        }
        // 実時間上限（--prompt かつ overall_timeout > 0 のときのみ）。無効時は
        // 決して解決しない future にして select! の他分岐に委ねる。
        _ = async {
            if is_prompt_mode && overall_timeout_secs > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(overall_timeout_secs)).await;
            } else {
                std::future::pending::<()>().await;
            }
        } => Outcome::OverallTimeout,
    };

    if let Some(handle) = monitor_handle {
        handle.abort();
    }

    let idle_timed_out = timed_out.load(std::sync::atomic::Ordering::SeqCst);
    let (result_text, result_line, ctrl_c_pressed, overall_timed_out): (
        Option<String>,
        Option<String>,
        bool,
        bool,
    ) = match outcome {
        Outcome::Completed(rt, rl) => (rt, rl, false, false),
        Outcome::CtrlC => (None, None, true, false),
        Outcome::OverallTimeout => (None, None, false, true),
    };
    let was_timed_out = idle_timed_out || overall_timed_out;

    if ctrl_c_pressed || was_timed_out {
        let _ = exec_child.kill().await;
        let _ = exec_child.wait().await;
        if was_timed_out {
            // ローカルの docker exec クライアントを kill してもコンテナ内の
            // claude プロセスには届かない。ワークスペースへの書き込みを確実に
            // 止めるためコンテナ内 claude も停止する。idle 監視タスクが発火した
            // 場合は既に実行済みだが、overall timeout 分岐を含めて冪等に実施する。
            // -x: 完全一致（/.claude/ パス等を誤 kill しない）
            tokio::process::Command::new("docker")
                .args(["exec", &ctx.container_name, "pkill", "-x", "claude"])
                .output()
                .await
                .ok();
        }
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
        let head_advanced = current_head != ctx.deferred_session.head_before;

        if head_advanced && was_dirty_before {
            // コミットは進んでいるが開始前から dirty: --mixed で HEAD だけ戻し、
            // working tree はそのまま保持（ユーザーの事前変更を消さない）
            std::process::Command::new("git")
                .args(["reset", "--mixed", &ctx.deferred_session.head_before])
                .current_dir(workspace_path)
                .output()
                .ok();
        } else if head_advanced {
            // コミットが進んでいて開始前はクリーン: --hard で完全復元
            crate::git::reset_hard(workspace_path, &ctx.deferred_session.head_before)?;
            crate::git::clean_fd(workspace_path)?;
        } else if !was_dirty_before {
            // HEAD は同じだが開始前クリーン: uncommitted changes を --hard で消す
            crate::git::reset_hard(workspace_path, &ctx.deferred_session.head_before)?;
            crate::git::clean_fd(workspace_path)?;
        }
        // was_dirty_before && !head_advanced: 開始前から dirty で HEAD 変更なし
        // → ユーザーの変更とエージェントの変更が混在。リセットしない（警告���み）
        ctx.store.mark_restored(&ctx.deferred_session.id)?;

        // idle（ストリーム無出力）と overall（実時間上限）で理由と秒数を出し分ける。
        let (limit_secs, kind_label) = if overall_timed_out {
            (overall_timeout_secs, "実時間が")
        } else {
            (idle_timeout_secs, "ストリーム無出力が")
        };
        let timeout_display = if limit_secs >= 60 {
            format!("{} 分", limit_secs / 60)
        } else {
            format!("{} 秒", limit_secs)
        };
        eprintln!();
        eprintln!(
            "⚠ {}{} を超えたため、セッションを中断しました。",
            kind_label, timeout_display
        );
        if let Some(ref p) = log_path {
            eprintln!("  ログ: {}", p.display());
        }
        if head_advanced || !was_dirty_before {
            eprintln!(
                "  作業ディレクトリを {} にリセットしました。",
                &ctx.deferred_session.head_before[..8.min(ctx.deferred_session.head_before.len())]
            );
        }
        if was_dirty_before {
            eprintln!(
                "  注意: セッション開始前に未コミットの変更がありました。エージェントの変更と混在している可能性があります。"
            );
            eprintln!(
                "  未追跡ファイルが残っている可能性があります。`git status` で確認してください。"
            );
        }
    }

    // コンテナの後処理
    if ctx.is_disposable {
        Command::new("docker")
            .args(["rm", "-f", &ctx.container_name])
            .output()
            .ok();
        // 使い捨てコンテナは runtime ディレクトリを丸ごと削除
        // （temp .claude.json と sanitized settings.json をまとめて掃除）。
        // ctx.runtime_dir は prepare.rs で必ず作成されるため、temp_claude_json
        // や sanitized settings.json の存在に関係なく確実に cleanup できる。
        std::fs::remove_dir_all(&ctx.runtime_dir).ok();
    } else if ctx.container_status != ContainerStatus::Running {
        Command::new("docker")
            .args(["stop", "-t", "10", &ctx.container_name])
            .output()
            .ok();
    }

    // ロック解放（コンテナ後処理が完了してから解放し、
    // 次の起動がコンテナ停止中に走る競合を防ぐ）
    drop(lock);

    // タイムアウトは「中途半端な成功」にしない: 後始末を終えたうえで非ゼロ終了
    // させ、呼び出し元（別 Claude セッション等）が失敗として扱えるようにする。
    // 詳細なリセット情報は上で stderr に出力済み。ここでは簡潔な理由と logs.txt
    // のパスを添えて返す。
    if was_timed_out {
        let logs_hint = log_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "N/A".to_string());
        let reason = if overall_timed_out {
            "実時間上限"
        } else {
            "ストリーム無出力"
        };
        anyhow::bail!(
            "セッションをタイムアウトで打ち切りました（{}）。ログ: {}",
            reason,
            logs_hint
        );
    }

    print_post_run_summary(
        opts,
        ctx,
        result_text.as_deref(),
        result_line.as_deref(),
        log_path.as_deref(),
        ctx.is_disposable,
    );

    Ok(())
}

fn print_post_run_summary(
    opts: &RunOptions,
    ctx: &RunContext,
    result_text: Option<&str>,
    result_line: Option<&str>,
    log_path: Option<&std::path::Path>,
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
        // 既定では生の stream-json を出さず、ここで簡潔な要約を組み立てて出す。
        let summary = crate::runtime::summarize_result_line(result_line);
        let (success, reason) = match &summary {
            Some(s) => {
                let reason = s.subtype.clone().unwrap_or_else(|| {
                    if s.is_error {
                        "error".to_string()
                    } else {
                        "success".to_string()
                    }
                });
                (!s.is_error, reason)
            }
            // result イベントが無い＝セッションが中断/クラッシュした可能性。
            None => (
                false,
                "no result reported (session interrupted or crashed)".to_string(),
            ),
        };
        let changed = crate::git::get_changed_files_since(
            std::path::Path::new(&ctx.effective_workspace),
            &ctx.deferred_session.head_before,
        );
        let logs_str = log_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "N/A".to_string());
        // git が失敗して変更ファイル一覧を算出できなかった場合は握りつぶさず、
        // 次のアクション（フルログ参照）が分かる注記を stderr に出す（指摘 #2）。
        // 要約側でも `(none)` とは別文言で表示される。
        if matches!(changed, crate::git::ChangedFiles::Unavailable) {
            eprintln!(
                "  Warning: could not compute the changed-file list (git command failed). \
                 Inspect the working tree manually or see the full logs: {}",
                logs_str
            );
        }
        let block = super::render_run_summary(success, &reason, result_text, &changed, &logs_str);
        println!();
        println!("{}", block);

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
        return;
    }

    // 以降は --resume（非 --prompt）パス。従来の ◇ スタイル表示を維持する。
    if let (Some(ref branch), Some(ref dir)) = (&ctx.worktree_branch_name, &ctx.worktree_dir_name) {
        println!("  ◇  Worktree: .worktrees/{}", dir);
        println!("  │  Branch: {}", branch);
        println!("  │  To review: cd .worktrees/{} && git diff main", dir);
        println!("  │  To merge:  git merge {}", branch);
        println!("  │  To remove: git worktree remove .worktrees/{}", dir);
    }

    println!("  {}", stopped_msg);
}
