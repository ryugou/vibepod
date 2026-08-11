use anyhow::{bail, Result};

use crate::runtime::{ContainerStatus, DockerRuntime};

pub async fn execute(name: Option<String>, all: bool) -> Result<()> {
    if name.is_none() && !all {
        bail!("Specify a container name or use --all to stop all VibePod containers");
    }

    let runtime = DockerRuntime::new().await?;

    if all {
        let containers = runtime.list_vibepod_containers().await?;
        if containers.is_empty() {
            println!("No VibePod containers found.");
            return Ok(());
        }
        let mut stopped = 0;
        for container in &containers {
            if should_stop(&container.state) {
                println!("Stopping {}...", container.name);
                runtime.stop_container(&container.name, 10).await?;
                stopped += 1;
            }
        }
        if stopped == 0 {
            println!("No running VibePod containers found.");
        } else {
            println!("Stopped {} container(s).", stopped);
        }
    } else if let Some(ref container_name) = name {
        if !container_name.starts_with("vibepod-") {
            bail!(
                "Container '{}' is not a VibePod container (name must start with 'vibepod-')",
                container_name
            );
        }
        // すでに停止済みの場合はスキップ（正常状態）
        let status = runtime.find_container_status(container_name).await?;
        match status {
            ContainerStatus::Running => {
                println!("Stopping {}...", container_name);
                runtime.stop_container(container_name, 10).await?;
                println!("Stopped.");
            }
            ContainerStatus::Stopped => {
                println!("Container {} is already stopped.", container_name);
            }
            ContainerStatus::None => {
                bail!("Container '{}' not found.", container_name);
            }
        }
    }

    Ok(())
}

/// `vibepod stop --all` の停止対象かどうかを `ContainerInfo.state` から
/// 判定する純関数。表示用の `status` 文字列（"Up 5 minutes" 等）は
/// `Restarting (...)` のような docker の実際の表記を捉え損ねるため使わない。
///
/// 停止対象は `running` / `restarting`。`paused` は対象外とする —
/// `docker stop` は paused コンテナにも効くが、一時停止はユーザーが
/// 明示的に行った状態であり、`stop --all` がそれを暗黙に解除して止めるのは
/// 意図と異なるため。
fn should_stop(state: &str) -> bool {
    matches!(state.to_lowercase().as_str(), "running" | "restarting")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_stop_running() {
        assert!(should_stop("running"));
    }

    #[test]
    fn should_stop_restarting() {
        assert!(should_stop("restarting"));
    }

    #[test]
    fn should_not_stop_paused() {
        assert!(!should_stop("paused"));
    }

    #[test]
    fn should_not_stop_exited() {
        assert!(!should_stop("exited"));
    }

    #[test]
    fn should_not_stop_created() {
        assert!(!should_stop("created"));
    }

    #[test]
    fn should_not_stop_dead() {
        assert!(!should_stop("dead"));
    }

    #[test]
    fn should_not_stop_unknown_state() {
        assert!(!should_stop("some-future-docker-state"));
    }
}
