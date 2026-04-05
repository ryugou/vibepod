use anyhow::Result;

use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    let runtime = DockerRuntime::new().await?;
    let containers = runtime.list_vibepod_containers().await?;
    if containers.is_empty() {
        println!("No running VibePod containers.");
        return Ok(());
    }

    let extract_project_fallback = |container_name: &str| -> String {
        let without_prefix = container_name
            .strip_prefix("vibepod-")
            .unwrap_or(container_name);
        if let Some(idx) = without_prefix.rfind('-') {
            let suffix = &without_prefix[idx + 1..];
            if (suffix.len() == 6 || suffix.len() == 8)
                && suffix.chars().all(|c| c.is_ascii_hexdigit())
            {
                return without_prefix[..idx].to_string();
            }
        }
        without_prefix.to_string()
    };

    struct ContainerInfo {
        name: String,
        project: String,
        workspace: Option<String>,
        status: String,
        elapsed: Option<String>,
        last_output: Option<String>,
    }

    let mut infos: Vec<ContainerInfo> = Vec::new();
    for (name, status) in &containers {
        let labels = runtime.get_container_labels(name).await.unwrap_or_default();
        let workspace = labels.get("vibepod.workspace").cloned();

        let project = if let Some(ref ws) = workspace {
            std::path::Path::new(ws)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(ws.as_str())
                .to_string()
        } else {
            extract_project_fallback(name)
        };

        let (elapsed, last_output) = if let Some(ref ws) = workspace {
            read_lock_times(ws)
        } else {
            (None, None)
        };

        infos.push(ContainerInfo {
            name: name.clone(),
            project,
            workspace,
            status: status.clone(),
            elapsed,
            last_output,
        });
    }

    let mut project_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for info in &infos {
        *project_counts.entry(info.project.clone()).or_insert(0) += 1;
    }

    println!(
        "{:<40} {:<20} {:<10} {:<12} STATUS",
        "CONTAINER", "PROJECT", "ELAPSED", "LAST OUTPUT"
    );
    for info in &infos {
        let project_display = if project_counts.get(&info.project).copied().unwrap_or(0) > 1 {
            if let Some(ref ws) = info.workspace {
                let parts: Vec<&str> = ws.split('/').collect();
                if parts.len() >= 2 {
                    format!("...{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
                } else {
                    info.project.clone()
                }
            } else {
                info.project.clone()
            }
        } else {
            info.project.clone()
        };

        let elapsed = info.elapsed.as_deref().unwrap_or("—");
        let last_output = info.last_output.as_deref().unwrap_or("—");

        println!(
            "{:<40} {:<20} {:<10} {:<12} {}",
            info.name, project_display, elapsed, last_output, info.status
        );
    }
    Ok(())
}

fn read_lock_times(workspace: &str) -> (Option<String>, Option<String>) {
    let lock_path = std::path::Path::new(workspace)
        .join(".vibepod")
        .join("prompt.lock");
    let content = match std::fs::read_to_string(&lock_path) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };

    let data: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };

    let now = chrono::Local::now();

    let elapsed = data["started_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| format_duration_since(now, dt.into()));

    let last_output = data["last_event_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| {
            let dur = format_duration_since(now, dt.into());
            format!("{} ago", dur)
        });

    (elapsed, last_output)
}

fn format_duration_since(
    now: chrono::DateTime<chrono::Local>,
    then: chrono::DateTime<chrono::Local>,
) -> String {
    let secs = (now - then).num_seconds().max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}
