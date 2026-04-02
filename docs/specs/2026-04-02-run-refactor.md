# run.rs リファクタリング（v1.4.0 スコープ）

## 背景

`src/cli/run.rs` の `execute()` 関数が 800 行超・引数 12 個に膨れている。
interactive / fire-and-forget / bridge の 3 モードが 1 関数に詰まっており、v2 で daemon/dashboard モードを追加する前に整理が必要。

## 変更方針

外部挙動は一切変えない。内部構造のみリファクタリング。

## 変更内容

### 1. `RunOptions` struct の導入

`execute()` の引数 12 個を構造体にまとめる。

```rust
pub struct RunOptions {
    pub resume: bool,
    pub prompt: Option<String>,
    pub no_network: bool,
    pub env_vars: Vec<String>,
    pub env_file: Option<String>,
    pub bridge: bool,
    pub notify_delay: u64,
    pub slack_channel: Option<String>,
    pub llm_provider: String,
    pub lang: Option<String>,
    pub worktree: bool,
    pub review: bool,
}
```

`execute()` のシグネチャを変更:

```rust
pub async fn execute(opts: RunOptions) -> Result<()> {
```

`src/main.rs` 側も `RunOptions` を構築して渡すように変更。

### 2. モード別サブ関数への分離

`execute()` から以下のサブ関数を切り出す:

#### `run_interactive()`
- interactive モード（`docker run -it`）の処理
- L720-783 相当

#### `run_fire_and_forget()`
- fire-and-forget モード（bollard API、stream_logs）の処理
- L786-882 相当
- `--prompt` 時の出力レイアウト（区切り線、Result 表示、diff サマリー、worktree 情報）を含む

#### `run_bridge()`
- bridge モード（Slack bridge）の処理
- L631-686 相当
- 現在 `execute()` 内で early return しているブロック

各サブ関数に必要なコンテキストは、共通の構造体（`RunContext` 等）としてまとめるか、必要な値を個別に渡す。

```rust
struct RunContext {
    runtime: DockerRuntime,
    container_name: String,
    effective_workspace: String,
    claude_args: Vec<String>,
    resolved_env_vars: Vec<String>,
    setup_cmd: Option<String>,
    temp_claude_json: Option<std::path::PathBuf>,
    global_config: config::GlobalConfig,
    home: std::path::PathBuf,
    worktree_branch_name: Option<String>,
    worktree_dir_name: Option<String>,
    lang_display: String,
}
```

#### `execute()` の分岐（リファクタリング後）

```rust
pub async fn execute(opts: RunOptions) -> Result<()> {
    // 1. 共通処理（git repo チェック、設定読み込み、Docker チェック、
    //    auth、env 解決、コンテナ名生成、worktree 作成、言語検出）
    let ctx = prepare_context(&opts).await?;

    // 2. モード別実行
    if opts.bridge {
        run_bridge(&opts, &ctx).await
    } else if interactive {
        run_interactive(&opts, &ctx).await
    } else {
        run_fire_and_forget(&opts, &ctx).await
    }
}
```

### 3. 共通処理の `prepare_context()` への切り出し

`execute()` の L98-628（モード判定〜コンテナ起動前の共通処理）を `prepare_context()` に切り出す。

bridge の env 解決は bridge 固有なので、`prepare_context()` ではなく `run_bridge()` 内に残す。
ただし `bridge_config` の構築が `execute()` の共通フローに組み込まれている場合は、
`prepare_context()` が `Option<BridgeConfig>` を返す形でもよい。

### 4. `build_container_config` の引数整理

現在の `build_container_config` も引数 9 個（`#[allow(clippy::too_many_arguments)]`）。
`RunContext` から必要なフィールドを取る形に変更する。

```rust
fn build_container_config(ctx: &RunContext, image: String, no_network: bool) -> ContainerConfig {
```

## 変更対象ファイル

- `src/cli/run.rs` — メイン変更対象
- `src/cli/mod.rs` — `RunOptions` の public 定義（`run.rs` 内でもよい）
- `src/main.rs` — `RunOptions` を構築して `execute` に渡す

## 検証

- `cargo check && cargo clippy` が通ること
- `cargo test` が通ること
- `#[allow(clippy::too_many_arguments)]` が不要になっていること

## 注意事項

- 外部挙動（出力メッセージ、コンテナ動作、オプション）は一切変更しない
- 関数の切り出しと構造体導入のみ
