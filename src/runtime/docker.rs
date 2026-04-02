use anyhow::{Context, Result};
use bollard::container::{
    AttachContainerOptions, AttachContainerResults, Config, CreateContainerOptions,
    ListContainersOptions, LogsOptions, RemoveContainerOptions, ResizeContainerTtyOptions,
    StartContainerOptions, StopContainerOptions, WaitContainerOptions,
};
use bollard::image::BuildImageOptions;
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;

#[derive(Clone)]
pub struct DockerRuntime {
    docker: Docker,
}

pub struct ContainerConfig {
    pub image: String,
    pub container_name: String,
    pub workspace_path: String,
    pub claude_json: Option<String>,
    pub gitconfig: Option<String>,
    pub args: Vec<String>,
    pub env_vars: Vec<String>,
    pub network_disabled: bool,
    pub setup_cmd: Option<String>,
}

impl DockerRuntime {
    pub async fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .context("Failed to connect to Docker. Is Docker Desktop or OrbStack running?")?;
        Ok(Self { docker })
    }

    pub async fn ping(&self) -> Result<()> {
        self.docker
            .ping()
            .await
            .context("Docker is not responding")?;
        Ok(())
    }

    pub async fn build_image(
        &self,
        dockerfile_content: &str,
        image_name: &str,
        build_args: HashMap<String, String>,
    ) -> Result<()> {
        let mut header = tar::Header::new_gnu();
        let dockerfile_bytes = dockerfile_content.as_bytes();
        header.set_path("Dockerfile")?;
        header.set_size(dockerfile_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        let mut tar_builder = tar::Builder::new(Vec::new());
        tar_builder.append(&header, dockerfile_bytes)?;
        let tar_data = tar_builder.into_inner()?;

        let options = BuildImageOptions {
            t: image_name,
            buildargs: build_args
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            ..Default::default()
        };

        let mut stream = self
            .docker
            .build_image(options, None, Some(tar_data.into()));

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => {
                    if let Some(stream) = output.stream {
                        print!("{}", stream);
                    }
                    if let Some(error) = output.error {
                        anyhow::bail!("Build error: {}", error);
                    }
                }
                Err(e) => anyhow::bail!("Build failed: {}", e),
            }
        }

        Ok(())
    }

    pub async fn image_exists(&self, image_name: &str) -> Result<bool> {
        match self.docker.inspect_image(image_name).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    pub async fn find_running_container(
        &self,
        name_prefix: &str,
    ) -> Result<Option<(String, String)>> {
        let options = ListContainersOptions::<String> {
            all: false,
            ..Default::default()
        };
        let containers = self.docker.list_containers(Some(options)).await?;
        for container in containers {
            if let Some(names) = &container.names {
                for name in names {
                    let clean_name = name.trim_start_matches('/').to_string();
                    if clean_name.starts_with(name_prefix) {
                        if let Some(id) = &container.id {
                            return Ok(Some((id.clone(), clean_name)));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    pub async fn create_and_start_container(&self, config: &ContainerConfig) -> Result<String> {
        let mut mounts = vec![Mount {
            target: Some("/workspace".to_string()),
            source: Some(config.workspace_path.clone()),
            typ: Some(MountTypeEnum::BIND),
            read_only: Some(false),
            ..Default::default()
        }];

        if let Some(ref claude_json_path) = config.claude_json {
            mounts.push(Mount {
                target: Some("/home/vibepod/.claude.json".to_string()),
                source: Some(claude_json_path.clone()),
                typ: Some(MountTypeEnum::BIND),
                read_only: Some(false),
                ..Default::default()
            });
        }

        if let Some(ref gitconfig_path) = config.gitconfig {
            mounts.push(Mount {
                target: Some("/home/vibepod/.gitconfig".to_string()),
                source: Some(gitconfig_path.clone()),
                typ: Some(MountTypeEnum::BIND),
                read_only: Some(true),
                ..Default::default()
            });
        }

        let host_config = HostConfig {
            mounts: Some(mounts),
            network_mode: if config.network_disabled {
                Some("none".to_string())
            } else {
                None
            },
            ..Default::default()
        };

        let mut env = config.env_vars.clone();
        env.push("TERM=xterm-256color".to_string());

        let cmd = if let Some(ref setup) = config.setup_cmd {
            let mut result = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("{} && exec \"$@\"", setup),
                "sh".to_string(),
            ];
            result.extend(config.args.clone());
            result
        } else {
            config.args.clone()
        };

        let container_config = Config {
            image: Some(config.image.clone()),
            cmd: Some(cmd),
            host_config: Some(host_config),
            env: Some(env),
            tty: Some(true),
            open_stdin: Some(true),
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: config.container_name.clone(),
            ..Default::default()
        };

        let response = self
            .docker
            .create_container(Some(options), container_config)
            .await
            .context("Failed to create container")?;

        self.docker
            .start_container(&response.id, None::<StartContainerOptions<String>>)
            .await
            .context("Failed to start container")?;

        Ok(response.id)
    }

    pub async fn stream_logs(&self, container_id: &str) -> Result<()> {
        let options = LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        let mut stream = self.docker.logs(container_id, Some(options));

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => print!("{}", output),
                Err(_) => {
                    // Stream closed — container exited, this is expected
                    break;
                }
            }
        }

        Ok(())
    }

    pub async fn stream_logs_formatted(&self, container_id: &str) -> Result<Option<String>> {
        let separator = "────────────────────────────────────────────────────────";
        println!("{}", separator);

        let options = LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        let mut stream = self.docker.logs(container_id, Some(options));
        let mut result_text: Option<String> = None;

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => {
                    let line = output.to_string();
                    let line = line.trim_end_matches('\n');
                    match serde_json::from_str::<serde_json::Value>(line) {
                        Ok(json) => {
                            let event_type =
                                json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match event_type {
                                "assistant" => {
                                    if let Some(contents) = json
                                        .get("message")
                                        .and_then(|m| m.get("content"))
                                        .and_then(|c| c.as_array())
                                    {
                                        for item in contents {
                                            match item.get("type").and_then(|t| t.as_str()) {
                                                Some("text") => {
                                                    if let Some(text) =
                                                        item.get("text").and_then(|t| t.as_str())
                                                    {
                                                        println!("  │  [assistant] {}", text);
                                                    }
                                                }
                                                Some("tool_use") => {
                                                    let name = item
                                                        .get("name")
                                                        .and_then(|n| n.as_str())
                                                        .unwrap_or("unknown");
                                                    let input = item
                                                        .get("input")
                                                        .cloned()
                                                        .unwrap_or(serde_json::Value::Null);
                                                    let input_display =
                                                        if let Some(obj) = input.as_object() {
                                                            let pairs: Vec<String> = obj
                                                                .iter()
                                                                .map(|(k, v)| {
                                                                    let val = v
                                                                        .as_str()
                                                                        .map(|s| {
                                                                            if s.len() > 80 {
                                                                                format!(
                                                                                    "\"{}...\"",
                                                                                    &s[..77]
                                                                                )
                                                                            } else {
                                                                                format!("\"{}\"", s)
                                                                            }
                                                                        })
                                                                        .unwrap_or_else(|| {
                                                                            v.to_string()
                                                                        });
                                                                    format!("{}: {}", k, val)
                                                                })
                                                                .collect();
                                                            format!("{{ {} }}", pairs.join(", "))
                                                        } else {
                                                            input.to_string()
                                                        };
                                                    println!(
                                                        "  │  [tool_use] {} {}",
                                                        name, input_display
                                                    );
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                "result" => {
                                    if let Some(result_val) =
                                        json.get("result").and_then(|r| r.as_str())
                                    {
                                        result_text = Some(result_val.to_string());
                                    }
                                }
                                "rate_limit_event" => {
                                    if let Some(info) = json.get("rate_limit_info") {
                                        let status = info
                                            .get("status")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or("");
                                        if status != "allowed" {
                                            let resets_at = info
                                                .get("resetsAt")
                                                .and_then(|r| r.as_str())
                                                .unwrap_or("");
                                            let limit_type = info
                                                .get("rateLimitType")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");
                                            println!("  │  [rate_limit] status: {}, resets_at: {}, type: {}", status, resets_at, limit_type);
                                        }
                                    }
                                }
                                _ => {
                                    // system, hook_started, hook_response, etc. — silently ignored
                                }
                            }
                        }
                        Err(_) => {
                            // Not valid JSON — pass through as-is
                            print!("{}", output);
                        }
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }

        println!("{}", separator);
        Ok(result_text)
    }

    pub async fn stop_container(&self, container_id: &str, timeout_secs: i64) -> Result<()> {
        let options = StopContainerOptions { t: timeout_secs };
        self.docker
            .stop_container(container_id, Some(options))
            .await
            .context("Failed to stop container")?;
        Ok(())
    }

    pub async fn remove_container(&self, container_id: &str) -> Result<()> {
        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };
        self.docker
            .remove_container(container_id, Some(options))
            .await
            .context("Failed to remove container")?;
        Ok(())
    }

    pub async fn attach_container(&self, container_id: &str) -> Result<AttachContainerResults> {
        let options = AttachContainerOptions::<String> {
            stdin: Some(true),
            stdout: Some(true),
            stderr: Some(true),
            stream: Some(true),
            ..Default::default()
        };
        let results = self
            .docker
            .attach_container(container_id, Some(options))
            .await
            .context("Failed to attach to container")?;
        Ok(results)
    }

    pub async fn resize_container_tty(
        &self,
        container_id: &str,
        width: u16,
        height: u16,
    ) -> Result<()> {
        let options = ResizeContainerTtyOptions { width, height };
        self.docker
            .resize_container_tty(container_id, options)
            .await
            .context("Failed to resize container TTY")?;
        Ok(())
    }

    pub async fn wait_container(&self, container_id: &str) -> Result<i64> {
        let options = WaitContainerOptions {
            condition: "not-running",
        };
        let mut stream = self.docker.wait_container(container_id, Some(options));
        if let Some(result) = stream.next().await {
            let response = result.context("Failed to wait for container")?;
            Ok(response.status_code)
        } else {
            Ok(0)
        }
    }
}
