use anyhow::Result;

use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    let runtime = DockerRuntime::new().await?;
    let containers = runtime.list_vibepod_containers().await?;
    if containers.is_empty() {
        println!("No running VibePod containers.");
        return Ok(());
    }
    println!("{:<40} {:<20} {:<20}", "CONTAINER", "PROJECT", "STATUS");
    for (name, status) in &containers {
        let project = name
            .trim_start_matches("vibepod-")
            .rsplit_once('-')
            .map(|(p, _)| p)
            .unwrap_or(name);
        println!("{:<40} {:<20} {:<20}", name, project, status);
    }
    Ok(())
}
