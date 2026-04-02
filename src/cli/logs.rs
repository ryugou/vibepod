use anyhow::{Context, Result};

use crate::runtime::DockerRuntime;

pub async fn execute(container: Option<String>, follow: bool, tail: String) -> Result<()> {
    let runtime = DockerRuntime::new().await?;

    let container_name = if let Some(name) = container {
        name
    } else {
        let containers = runtime.list_vibepod_containers().await?;
        containers
            .into_iter()
            .next()
            .map(|(name, _)| name)
            .context("No VibePod containers found. Run `vibepod ps` to check.")?
    };

    let container_id = runtime
        .find_container_by_name(&container_name)
        .await?
        .with_context(|| format!("Container '{}' not found", container_name))?;

    if follow {
        runtime.stream_logs(&container_id).await?;
    } else {
        runtime.get_logs(&container_id, &tail).await?;
    }
    Ok(())
}
