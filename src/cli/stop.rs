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
        for (container_name, status) in &containers {
            if status.starts_with("Up") || status.to_lowercase().contains("running") {
                println!("Stopping {}...", container_name);
                runtime.stop_container(container_name, 10).await?;
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
