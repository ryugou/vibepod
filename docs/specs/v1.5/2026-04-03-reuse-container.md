# --reuse オプション + vibepod rm + Copilot レビュー指摘対応

## 概要

コンテナの再利用オプションと削除コマンドの追加。加えて、PR #28 の Copilot レビュー指摘を同時に対応する。

## 1. --reuse オプション

### 動作

- `vibepod run --reuse` — コンテナ終了後もコンテナを削除しない
- 次回 `vibepod run` 時に同じプロジェクトの既存コンテナが見つかれば、`docker start` + `docker exec -it` で再接続
- `--reuse` なし（デフォルト）の場合は従来通り毎回コンテナを削除

### CLI 変更

`src/cli/mod.rs` の Run サブコマンドに追加:
```rust
/// Reuse container across runs (skip setup on subsequent runs)
#[arg(long)]
reuse: bool,
```

`src/main.rs` で `reuse` を `RunOptions` に渡す。

### 実装変更

`src/cli/run/mod.rs` の `RunOptions` に `pub reuse: bool` を追加。

`src/cli/run/interactive.rs`:
- `--reuse` の場合、`docker run` から `--rm` を外す
- 既存コンテナがある場合: `docker start {name}` → `docker exec -it {name} claude {args}` で再接続
- 既存コンテナの検出は `docker ps -a --filter "name={container_name}" --format "{{.ID}}"` で行う（プロジェクト名ベースで検索）

`src/cli/run/prompt.rs`:
- `--reuse` の場合、終了後に `docker stop` / `docker rm` をスキップ
- 既存コンテナがある場合: `docker start {name}` → `docker logs --follow` で再接続

`src/cli/run/prepare.rs`:
- `--reuse` の場合、既存コンテナの検出ロジックを変更。既存コンテナが見つかったら「再利用」として RunContext にフラグを立てる（setup_cmd をスキップするため）
- 既存コンテナの名前は `vibepod-{project_name}-reuse` のような固定名にする（ランダムハッシュだと検索できないため）

`src/runtime/docker.rs`:
- `start_container` メソッド追加: `docker start {container_id}`
- `stop_container` メソッド追加: `docker stop {container_id}`
- `remove_container` メソッド追加: `docker rm -f {container_id}`

## 2. vibepod rm コマンド

### 動作

- `vibepod rm <name>` — 指定コンテナを削除
- `vibepod rm --all` — 全 vibepod コンテナを削除

### CLI 変更

`src/cli/mod.rs` に Rm サブコマンド追加:
```rust
/// Remove VibePod containers
Rm {
    /// Container name (or use --all)
    name: Option<String>,
    /// Remove all VibePod containers
    #[arg(long)]
    all: bool,
},
```

`src/cli/rm.rs` を新規作成:
- `name` 指定時: `docker rm -f {name}`
- `--all` 時: `docker ps -a --filter "name=vibepod-"` で一覧取得 → 全部 `docker rm -f`

`src/main.rs` に Rm のルーティング追加。

## 3. Copilot レビュー指摘対応

### 3-1. std::process::Command → tokio::process::Command (docker.rs)

async メソッド内でブロッキング Command を使っている。`tokio::process::Command` に変更する。
`use std::process::{Command, Stdio}` → `use tokio::process::Command; use std::process::Stdio;`

注意: `tokio::process::Command` は `.output()` と `.status()` が async になる。全ての呼び出し箇所に `.await` を追加すること。

### 3-2. build_image の固定 temp ディレクトリ (docker.rs)

`std::env::temp_dir().join("vibepod-build")` → `tempfile::tempdir()` で一意な temp dir を使う。
`tempfile` クレートが Cargo.toml の `[dev-dependencies]` にあるが、`[dependencies]` に移動する必要がある。

### 3-3. get_logs の exit status 無視 (docker.rs)

`docker logs` の exit status をチェックし、失敗時はエラーを返す。

### 3-4. stream_logs の exit status 無視 (docker.rs)

同上。

### 3-5. prompt.rs の docker logs exit status 未チェック

`run_fire_and_forget` 内の `docker logs --follow` の exit status を確認。Ctrl+C でなければエラーを返す。

## 完了条件

- `cargo fmt && cargo clippy` が通る
- `cargo test` が通る
- `cargo build --release` が成功
- `./target/release/vibepod run --help` に `--reuse` がある
- `./target/release/vibepod rm --help` が動作する

## コミット

- codex review を実行（`codex review -c sandbox_mode=danger-full-access -c approval_policy=never`、timeout: 600000）
- 指摘がなくなるまで修正（最大 5 回）
- Conventional Commits 準拠でコミット: `feat: add --reuse option and vibepod rm command`
- `git push -u origin feat/reuse-container`
- `gh pr create --base main`
