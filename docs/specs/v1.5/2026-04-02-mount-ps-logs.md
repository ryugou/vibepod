# --mount, vibepod ps, vibepod logs の実装

## 1. `--mount` オプション

ホストのファイルやディレクトリを read-only でコンテナにマウントする。

### CLI 引数

`src/cli/mod.rs`:
```rust
/// Mount host file/directory into container (read-only). Repeatable.
/// Format: <host-path>:<container-path> or <host-path> (mounted to same path under /mnt/)
#[arg(long, num_args = 1)]
mount: Vec<String>,
```

### 動作

- `--mount /path/to/spec.md:/workspace/spec.md` → 指定パスにマウント
- `--mount /path/to/spec.md` → `/mnt/spec.md` にマウント（コンテナパス省略時）
- 複数指定可
- すべて read-only

### 実装箇所

- `src/cli/mod.rs` — CLI 引数追加
- `src/main.rs` — 引数の受け渡し
- `src/cli/run.rs` — `RunOptions` に `mount` フィールド追加、`prepare_context()` でパース
- `src/runtime/docker.rs` — `ContainerConfig` に `extra_mounts: Vec<(String, String)>` 追加、マウント生成

### パースロジック

`run.rs` に `parse_mount_arg()` を追加:
```rust
fn parse_mount_arg(arg: &str) -> Result<(String, String)> {
    if let Some((host, container)) = arg.split_once(':') {
        Ok((host.to_string(), container.to_string()))
    } else {
        let path = std::path::Path::new(arg);
        let filename = path.file_name()
            .context("Invalid mount path")?
            .to_string_lossy();
        Ok((arg.to_string(), format!("/mnt/{}", filename)))
    }
}
```

### 起動時表示

```
Mount: /path/to/project → /workspace
Mount (ro): /path/to/spec.md → /mnt/spec.md
```

## 2. `vibepod ps` コマンド

実行中の vibepod コンテナの一覧を表示する。

### CLI 定義

`src/cli/mod.rs`:
```rust
/// List running VibePod containers
Ps {},
```

### 実装

`src/cli/ps.rs` — 新規ファイル:

```rust
pub async fn execute() -> Result<()> {
    let runtime = DockerRuntime::new().await?;
    let containers = runtime.list_vibepod_containers().await?;
    if containers.is_empty() {
        println!("No running VibePod containers.");
        return Ok(());
    }
    println!("{:<40} {:<20} {:<20}", "CONTAINER", "PROJECT", "STATUS");
    for (name, status) in &containers {
        let project = name.trim_start_matches("vibepod-")
            .rsplit_once('-')
            .map(|(p, _)| p)
            .unwrap_or(name);
        println!("{:<40} {:<20} {:<20}", name, project, status);
    }
    Ok(())
}
```

### DockerRuntime に追加

`src/runtime/docker.rs`:
```rust
pub async fn list_vibepod_containers(&self) -> Result<Vec<(String, String)>> {
    let options = ListContainersOptions::<String> {
        all: true,
        ..Default::default()
    };
    let containers = self.docker.list_containers(Some(options)).await?;
    let mut result = Vec::new();
    for container in containers {
        if let Some(names) = &container.names {
            for name in names {
                let clean = name.trim_start_matches('/').to_string();
                if clean.starts_with("vibepod-") {
                    let status = container.status.clone().unwrap_or_default();
                    result.push((clean, status));
                }
            }
        }
    }
    Ok(result)
}
```

## 3. `vibepod logs` コマンド

指定したコンテナのログを表示する。コンテナ名を省略した場合は最新のコンテナを対象にする。

### CLI 定義

`src/cli/mod.rs`:
```rust
/// Show logs of a VibePod container
Logs {
    /// Container name (defaults to most recent)
    #[arg()]
    container: Option<String>,
    /// Follow log output
    #[arg(short, long)]
    follow: bool,
    /// Number of lines to show from the end
    #[arg(short = 'n', long, default_value = "100")]
    tail: String,
},
```

### 実装

`src/cli/logs.rs` — 新規ファイル:

```rust
pub async fn execute(container: Option<String>, follow: bool, tail: String) -> Result<()> {
    let runtime = DockerRuntime::new().await?;

    let container_name = if let Some(name) = container {
        name
    } else {
        // 最新の vibepod コンテナを取得
        let containers = runtime.list_vibepod_containers().await?;
        containers.first()
            .map(|(name, _)| name.clone())
            .context("No VibePod containers found. Run `vibepod ps` to check.")?
    };

    let container_id = runtime.find_container_by_name(&container_name).await?
        .context(format!("Container '{}' not found", container_name))?;

    if follow {
        runtime.stream_logs(&container_id).await?;
    } else {
        runtime.get_logs(&container_id, &tail).await?;
    }
    Ok(())
}
```

### DockerRuntime に追加

`src/runtime/docker.rs`:
```rust
pub async fn find_container_by_name(&self, name: &str) -> Result<Option<String>> {
    // name でコンテナを検索して ID を返す
}

pub async fn get_logs(&self, container_id: &str, tail: &str) -> Result<()> {
    let options = LogsOptions::<String> {
        follow: false,
        stdout: true,
        stderr: true,
        tail: tail.to_string(),
        ..Default::default()
    };
    let mut stream = self.docker.logs(container_id, Some(options));
    while let Some(result) = stream.next().await {
        match result {
            Ok(output) => print!("{}", output),
            Err(_) => break,
        }
    }
    Ok(())
}
```

## 変更対象ファイル

- `Cargo.toml` — バージョンはそのまま（1.5.0）
- `src/cli/mod.rs` — `Ps`, `Logs` コマンド追加、`--mount` オプション追加
- `src/cli/ps.rs` — 新規
- `src/cli/logs.rs` — 新規
- `src/main.rs` — `Ps`, `Logs` のディスパッチ追加、`mount` 引数の受け渡し
- `src/cli/run.rs` — `RunOptions` に `mount` 追加、`parse_mount_arg()`、起動時表示
- `src/runtime/docker.rs` — `extra_mounts`、`list_vibepod_containers()`、`find_container_by_name()`、`get_logs()`
- `tests/cli_test.rs` — `--mount`、`ps`、`logs` のパーステスト追加
- `tests/run_logic_test.rs` — `parse_mount_arg()` のテスト追加

## 検証

- `cargo fmt && cargo check && cargo clippy && cargo test` が通ること
