use anyhow::Result;

use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    let runtime = DockerRuntime::new().await?;
    let containers = runtime.list_vibepod_containers().await?;
    if containers.is_empty() {
        println!("No running VibePod containers.");
        return Ok(());
    }

    // コンテナ名からプロジェクト名を抽出するフォールバック（ラベルがない場合）。
    // 新形式: vibepod-{name}-{8hex}（末尾が正確に 8 文字の hex）
    // 旧ランダム形式: vibepod-{name}-{6hex}（末尾が正確に 6 文字の hex）
    let extract_project_fallback = |container_name: &str| -> String {
        let without_prefix = container_name
            .strip_prefix("vibepod-")
            .unwrap_or(container_name);
        if let Some(idx) = without_prefix.rfind('-') {
            let suffix = &without_prefix[idx + 1..];
            // 正確に 6 文字または 8 文字の hex のみハッシュとして扱う
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
    }

    let mut infos: Vec<ContainerInfo> = Vec::new();
    for (name, status) in &containers {
        // ラベルからワークスペースパスを取得し、プロジェクト名を決定する
        let labels = runtime.get_container_labels(name).await.unwrap_or_default();
        let workspace = labels.get("vibepod.workspace").cloned();

        // ラベルがあればワークスペースの最終コンポーネントをプロジェクト名に使用
        let project = if let Some(ref ws) = workspace {
            std::path::Path::new(ws)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(ws.as_str())
                .to_string()
        } else {
            extract_project_fallback(name)
        };

        infos.push(ContainerInfo {
            name: name.clone(),
            project,
            workspace,
            status: status.clone(),
        });
    }

    // 同名プロジェクトが複数あるか確認
    let mut project_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for info in &infos {
        *project_counts.entry(info.project.clone()).or_insert(0) += 1;
    }

    println!("{:<45} {:<30} STATUS", "CONTAINER", "PROJECT");
    for info in &infos {
        let project_display = if project_counts.get(&info.project).copied().unwrap_or(0) > 1 {
            // 同名プロジェクトがある: パスを省略して表示
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
        println!("{:<45} {:<30} {}", info.name, project_display, info.status);
    }
    Ok(())
}
