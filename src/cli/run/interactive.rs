use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::runtime::{ContainerStatus, DockerRuntime};

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

pub(super) async fn run_interactive(opts: &RunOptions, ctx: &RunContext) -> Result<()> {
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

    // セッションを記録（コンテナ起動直前）
    ctx.store.add(ctx.deferred_session.clone())?;

    println!("  ◇  Container: {}", ctx.container_name);
    println!("  └\n");

    let runtime = DockerRuntime::new().await?;

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
            runtime.start_container(&ctx.container_name).await?;
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

    // docker exec -it -e TOKEN=... {name} bash --login -c 'exec claude "$@"' -- {args}
    // bash --login でログインプロファイルをソースし、setup で設定した PATH を引き継ぐ
    let mut exec_args = vec!["exec".to_string(), "-it".to_string()];
    for env_var in &ctx.exec_env_vars {
        exec_args.push("-e".to_string());
        exec_args.push(env_var.clone());
    }
    exec_args.push(ctx.container_name.clone());
    exec_args.push("bash".to_string());
    exec_args.push("--login".to_string());
    exec_args.push("-c".to_string());
    exec_args.push(r#"exec claude "$@""#.to_string());
    exec_args.push("--".to_string()); // $0 placeholder（"$@" は $1 以降）
    exec_args.extend(ctx.claude_args.clone());

    let status = Command::new("docker")
        .args(&exec_args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to exec into container")?;

    // Claude の終了コードは無視（ユーザーが終了した場合など）
    let _ = status;

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
        println!("  Container stopped and removed.");
    } else if ctx.container_status == ContainerStatus::Running {
        // 元から実行中だったコンテナ: 停止しない（並行 exec の他セッションが残る可能性）
        println!("  Disconnected from container (still running).");
    } else {
        // 停止中または新規作成したコンテナ: 停止して保持
        Command::new("docker")
            .args(["stop", "-t", "10", &ctx.container_name])
            .output()
            .ok();
        println!("  Container stopped (container preserved for next run).");
    }

    Ok(())
}
