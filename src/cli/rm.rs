use anyhow::{bail, Result};

use crate::runtime::DockerRuntime;

pub async fn execute(name: Option<String>, all: bool) -> Result<()> {
    if name.is_none() && !all {
        bail!("Specify a container name or use --all to remove all VibePod containers");
    }

    let runtime = DockerRuntime::new().await?;

    if all {
        let containers = runtime.list_vibepod_containers().await?;
        if containers.is_empty() {
            println!("No VibePod containers found.");
            return Ok(());
        }
        for (container_name, _status) in &containers {
            println!("Removing {}...", container_name);
            runtime.remove_container(container_name).await?;
        }
        println!("Removed {} container(s).", containers.len());
    } else if let Some(ref container_name) = name {
        if !container_name.starts_with("vibepod-") {
            bail!(
                "Container '{}' is not a VibePod container (name must start with 'vibepod-')",
                container_name
            );
        }
        println!("Removing {}...", container_name);
        runtime.remove_container(container_name).await?;
        println!("Removed.");
    }

    Ok(())
}
