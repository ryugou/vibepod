use anyhow::{Context, Result};
use std::collections::HashMap;

use crate::config::{self, GlobalConfig};
use crate::runtime::DockerRuntime;
use crate::ui::sanitize::sanitize_single_line;
use crate::ui::{banner, prompts};

pub async fn execute() -> Result<()> {
    banner::print_banner();

    // 1. Check Docker
    let runtime = DockerRuntime::new()
        .await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;
    runtime.ping().await?;

    // 2. Select agent
    let agent = prompts::select_agent()?;

    // 3. Build image
    let image_name = format!("vibepod-{}:latest", agent);

    println!("\n  Building Docker image: {}...", image_name);

    let dockerfile = include_str!("../../templates/Dockerfile");

    #[cfg(unix)]
    let (uid, gid) = {
        // SAFETY: getuid() and getgid() are simple syscalls with no preconditions
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        (uid, gid)
    };
    #[cfg(not(unix))]
    let (uid, gid): (u32, u32) = (1000, 1000);

    let mut build_args = HashMap::new();
    build_args.insert("HOST_UID".to_string(), uid.to_string());
    build_args.insert("HOST_GID".to_string(), gid.to_string());

    match runtime
        .build_image(dockerfile, &image_name, build_args)
        .await
    {
        Ok(_) => {}
        Err(e) => {
            eprintln!("\n  ✗ Build failed: {}", e);
            eprintln!("    Check your network connection and try `vibepod init` again.");
            return Err(e);
        }
    }

    // 4. イメージ再ビルド後に既存のコンテナを全削除する（config 保存前に行う）
    //    running コンテナがある場合は確認プロンプトを表示（非インタラクティブ時は強制削除）
    let containers = runtime.list_vibepod_containers().await?;
    if !containers.is_empty() {
        let running_count = containers
            .iter()
            .filter(|(_, status)| {
                status.starts_with("Up") || status.to_lowercase().contains("running")
            })
            .count();

        let should_remove =
            if running_count > 0 && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                // インタラクティブ: 確認プロンプト
                prompts::confirm_remove_all_containers(containers.len(), running_count)?
            } else {
                // 非インタラクティブまたは停止済みのみ: 強制削除
                if running_count > 0 {
                    eprintln!(
                        "  Warning: Forcibly removing {} running container(s) \
                     (non-interactive mode).",
                        running_count
                    );
                }
                true
            };

        if should_remove {
            println!("  Removing {} existing container(s)...", containers.len());
            for (container_name, _) in &containers {
                runtime.remove_container(container_name).await?;
            }
            println!("  Removed {} container(s).", containers.len());
        } else {
            // ユーザーがコンテナ削除を拒否 → config を更新しない（旧コンテナが旧イメージのまま残る）
            eprintln!(
                "  Skipping config update: existing containers were not removed. \
                 Re-run `vibepod init` and remove containers to apply the new image."
            );
            return Ok(());
        }
    }

    // 5. Save config（コンテナ削除後に保存することで、削除キャンセル時に旧イメージが残ったまま
    //    config が更新される問題を回避する）
    let config_dir = config::default_config_dir()?;
    let config = GlobalConfig {
        default_agent: agent,
        image: image_name,
    };
    config::save_global_config(&config, &config_dir)?;

    // 6. ECC cache initialization / refresh (idempotent).
    {
        let ecc_cfg = config::load_ecc_config(&config_dir)?;
        ecc_cfg.validate()?;
        let cache = crate::ecc::cache_dir(&config_dir);

        if cache.join(".git").exists() {
            println!("\n  Refreshing ecc cache at {}...", cache.display());
            match crate::ecc::fetch_latest(&config_dir, &ecc_cfg) {
                Ok(_) => println!("  Refreshed."),
                Err(e) => eprintln!(
                    "  Warning: ecc refresh failed: {e}\n  \
                     Run `vibepod template update` later to retry."
                ),
            }
        } else {
            println!(
                "\n  Cloning ecc ({}@{}) into {}...",
                sanitize_single_line(&ecc_cfg.repo, 500),
                sanitize_single_line(&ecc_cfg.r#ref, 200),
                cache.display()
            );
            crate::ecc::ensure_cloned(&config_dir, &ecc_cfg)?;
            println!("  Cloned.");
        }
    }

    println!("\n  Done! Run `vibepod run` in any git repo to start.\n");

    Ok(())
}
