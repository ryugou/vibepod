use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::runtime::{ContainerStatus, DockerRuntime};

use super::{
    build_container_config, sync_codex_stage_after_run, ContainerLiveness, RunContext, RunOptions,
};

/// コンテナを作成してセットアップを実行する（初回フロー）。
/// セットアップ失敗時はコンテナを自動削除してエラーを返す。
async fn create_and_setup(ctx: &RunContext, opts: &RunOptions) -> Result<()> {
    // この関数はコンテナを実際に作る唯一の地点であり、初回作成・`--new` 後・
    // `vibepod rm` 後・**setup marker 欠落による作り直し**のすべてがここを
    // 通る。`prepare_context` の時点で `container_status` から「これから
    // 新規作成するか」を予測すると、呼び出し側（この関数の呼び出し元）が
    // 既存コンテナを削除してから作り直す marker 欠落の経路を取りこぼすため、
    // 予測ではなく実際の作成地点でステージをリセットする。
    super::reset_plugins_data_stage(&ctx.runtime_dir)?;

    let container_config =
        build_container_config(ctx, ctx.effective_image.clone(), opts.no_network);
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

    // `container_created` は「このコンテナが今この run で作られたか」。
    // 新規作成直後の Claude Code はイメージに焼かれたバージョン（=
    // 最後の `vibepod init` 時点）なので、更新チェックの throttle を
    // 無視して必ずチェックさせる必要がある。
    let container_created = match ctx.container_status {
        ContainerStatus::Running => {
            // 実行中でもセットアップ完了マーカーを確認する
            // （初回起動途中で中断された場合にセットアップ未完了のまま残ることがある）
            if !runtime.check_setup_marker(&ctx.container_name).await? {
                // マーカーなし: セットアップ未完了 → コンテナ削除して初回フローへ
                runtime.remove_container(&ctx.container_name).await?;
                create_and_setup(ctx, opts).await?;
                true
            } else {
                false
            }
        }
        ContainerStatus::Stopped => {
            // 停止中のコンテナを起動してマーカー確認
            runtime.start_container(&ctx.container_name).await?;
            if !runtime.check_setup_marker(&ctx.container_name).await? {
                // マーカーなし: セットアップ未完了 → コンテナ削除して初回フローへ
                runtime.remove_container(&ctx.container_name).await?;
                create_and_setup(ctx, opts).await?;
                true
            } else {
                false
            }
        }
        ContainerStatus::None => {
            // コンテナなし: 新規作成してセットアップ
            create_and_setup(ctx, opts).await?;
            true
        }
    };

    // コンテナ内 Claude Code の更新（失敗しても続行する。詳細は update モジュール）
    crate::update::maybe_update_claude(
        &ctx.config_dir,
        &ctx.container_name,
        opts.update_policy,
        opts.no_network,
        container_created,
    )
    .await;

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

    // codex ステージ→store の同期は liveness に関わらず JSON 完全性検証付きで
    // 行う(round 11 P1 / round 12 P1-a)。disposable 経路では docker rm -f の
    // exit status を確認し、削除成功が確認できた場合のみ runtime dir(bind mount
    // 中のステージを含む)を削除する。失敗時は稼働継続の可能性があるため、失敗内容と
    // 手動対処を stderr に出して runtime dir を保持する(round 12 P1-b)。
    if ctx.is_disposable {
        // 使い捨てコンテナ（--worktree）: 削除。
        let removal = Command::new("docker")
            .args(["rm", "-f", &ctx.container_name])
            .output();
        let liveness = match &removal {
            Ok(o) if o.status.success() => ContainerLiveness::Stopped,
            other => {
                let detail = match other {
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        let stderr = stderr.trim();
                        if stderr.is_empty() {
                            format!("docker rm -f exited with status {}", o.status)
                        } else {
                            stderr.to_string()
                        }
                    }
                    Err(e) => e.to_string(),
                };
                eprintln!(
                    "warning: failed to remove disposable container {name}: {detail}. \
                     The container may still be running; run `vibepod rm {name}` to remove it \
                     manually. Preserving the codex auth stage and runtime dir for the next run.",
                    name = ctx.container_name
                );
                ContainerLiveness::Running
            }
        };
        // finalize は codex ステージを検証付きで同期したうえで、Stopped のときだけ
        // runtime ディレクトリ(temp .claude.json と sanitized settings.json、
        // およびステージ)を丸ごと削除する。ステージを含むため、同期より前に消すと
        // 書き戻せなくなる順序を finalize 側が担保する。ctx.runtime_dir は prepare.rs
        // で必ず作成されるため、これらの temp ファイルの有無に関係なく cleanup できる。
        super::finalize_disposable_runtime_dir(
            &ctx.runtime_dir,
            &ctx.config_dir,
            ctx.codex_dir.is_some(),
            liveness,
        );
        if liveness == ContainerLiveness::Stopped {
            println!("  Container stopped and removed.");
        } else {
            println!(
                "  Preserved runtime dir; remove the container manually and re-run to clean up."
            );
        }
    } else if ctx.container_status == ContainerStatus::Running {
        // 元から実行中だったコンテナ: 停止しない（並行 exec の他セッションが残る
        // 可能性）。runtime dir は削除しないので、稼働中のステージを検証付きで
        // 同期するだけでよい。
        sync_codex_stage_after_run(ctx);
        println!("  Disconnected from container (still running).");
    } else {
        // 停止中または新規作成したコンテナ: 停止して保持。stop の exit status を
        // 確認し、失敗時は失敗内容と手動対処を stderr に出す。runtime dir は
        // どちらにせよ削除しないため、成否に関わらず最後に検証付きで同期する。
        let mut stop_succeeded = false;
        let stop = Command::new("docker")
            .args(["stop", "-t", "10", &ctx.container_name])
            .output();
        match &stop {
            Ok(o) if o.status.success() => {
                stop_succeeded = true;
            }
            other => {
                let detail = match other {
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        let stderr = stderr.trim();
                        if stderr.is_empty() {
                            format!("docker stop exited with status {}", o.status)
                        } else {
                            stderr.to_string()
                        }
                    }
                    Err(e) => e.to_string(),
                };
                eprintln!(
                    "warning: failed to stop container {name}: {detail}. \
                     It may still be running; run `vibepod rm {name}` to remove it manually.",
                    name = ctx.container_name
                );
            }
        }
        sync_codex_stage_after_run(ctx);
        if stop_succeeded {
            println!("  Container stopped (container preserved for next run).");
        } else {
            println!(
                "  Container state unconfirmed (stop failed; see warning above). Container preserved."
            );
        }
    }

    Ok(())
}
