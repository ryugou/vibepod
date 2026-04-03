use anyhow::Result;

use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    let runtime = DockerRuntime::new().await?;
    let containers = runtime.list_vibepod_containers().await?;
    if containers.is_empty() {
        println!("No running VibePod containers.");
        return Ok(());
    }
    println!("{:<45} {:<25} STATUS", "CONTAINER", "PROJECT");
    for (name, status) in &containers {
        let project = name
            .strip_prefix("vibepod-")
            .and_then(|rest| rest.rsplit_once('-'))
            .map(|(p, _)| p)
            .unwrap_or(name);
        println!("{:<45} {:<25} {}", name, project, status);
    }
    Ok(())
}
