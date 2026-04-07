# --reuse Copilot レビュー指摘対応

## 概要

PR #30 の Copilot レビュー指摘への対応。

## 1. wait_for_reuse_setup のコンテナ exit 検知

`src/cli/run/prompt.rs` の `wait_for_reuse_setup` を修正。

現状: `docker logs --follow` で `VIBEPOD_SETUP_DONE` マーカーが出るまで無限に待つ。setup が失敗してコンテナが死んだ場合、無限待ちになる。

修正: `docker logs --follow` のストリームが閉じた（コンテナが exit した）のにマーカーが出ていない場合、setup 失敗としてエラーを返す。ログストリームの終了は while ループが自然に抜けることで検知できる。ループ後に「マーカーが見つかったか」をフラグで判定する。

```rust
async fn wait_for_reuse_setup(container_name: &str) -> Result<()> {
    // ... docker logs --follow を開始
    let mut found_marker = false;
    while let Ok(Some(line)) = lines.next_line().await {
        println!("{}", line);
        if line.contains("VIBEPOD_SETUP_DONE") {
            found_marker = true;
            break;
        }
    }
    let _ = child.kill().await;
    if !found_marker {
        bail!("Container setup failed: VIBEPOD_SETUP_DONE marker was not found. Check the setup output above for errors.");
    }
    Ok(())
}
```

## 2. ドキュメント更新

### README.md
- `vibepod run` のオプション表に `--reuse` を追加（説明: Reuse container across runs to skip setup on subsequent runs）
- コマンド一覧に `vibepod rm` を追加（説明: Remove VibePod containers）
- `vibepod rm` の引数表（`<name>`: 削除するコンテナ名、`--all`: 全 VibePod コンテナを削除）

### docs/design.md
- vibepod run のフロー説明に `--reuse` の動作を追記（初回: setup 実行 + コンテナ保持、2回目以降: docker start + docker exec で再接続、setup スキップ）

## 3. ユニットテスト追加

`tests/run_logic_test.rs` に以下のテストを追加:

### to_docker_args のテスト
- `test_to_docker_args_interactive`: interactive=true で `-it --rm` が含まれる
- `test_to_docker_args_detached`: interactive=false で `-d` が含まれる
- `test_to_docker_args_reuse`: reuse=true で `--rm` が含まれない、コンテナ名に `-reuse` が含まれる
- `test_to_docker_args_env_vars`: 環境変数が `-e` フラグで正しく渡される
- `test_to_docker_args_setup_cmd`: setup_cmd がある場合、`sh -c` ラッパーが付く

### vibepod rm のテスト
- `test_rm_rejects_non_vibepod_prefix`: `vibepod-` で始まらないコンテナ名を拒否する

注意: ContainerConfig と to_docker_args が `src/runtime/docker.rs` に定義されているので、テストは `tests/docker_test.rs` に追加する方が適切かもしれない。既存のテストファイル構成を確認してから決めること。

## 4. prepare.rs の std::process::Command を DockerRuntime 経由に統一

`src/cli/run/prepare.rs` の既存コンテナの stop/rm 処理が `std::process::Command` を直接使っている。DockerRuntime に `stop_container` と `remove_container` メソッドがあるので、それを使うように修正。

```rust
// Before:
let stop = Command::new("docker").args(["stop", "-t", "10", &existing_id]).output()?;
// After:
runtime.stop_container(&existing_id, 10).await?;
runtime.remove_container(&existing_id).await?;
```

## 5. docker logs 終了コードの扱い

`src/cli/run/prompt.rs` の `run_fire_and_forget` で、`log_child.kill()` 後の `wait()` が SIGKILL による非ゼロ終了を返す。シグナル終了（exit code なし）は正常扱いにする。

```rust
if !ctrl_c_pressed {
    if let Ok(status) = exit_status {
        // Signal-killed processes (e.g., after our kill()) have no exit code on Unix
        if let Some(code) = status.code() {
            if code != 0 {
                bail!("docker logs exited with code {} for container {}", code, ctx.container_name);
            }
        }
        // No exit code (killed by signal) is expected after kill()
    }
}
```

## 完了条件

- `cargo fmt && cargo clippy` が通る
- `cargo test` が通る（新規テスト含む）
- `cargo build --release` が成功

## コミット

- codex review を実行（`codex review -c sandbox_mode=danger-full-access -c approval_policy=never`、timeout: 600000）
- 指摘がなくなるまで修正（最大 5 回）
- Conventional Commits 準拠でコミット: `fix: address reuse review findings - exit detection, docs, tests`
- `git push origin feat/reuse-container`
- PR 作成は不要（既存の PR #30 に追加される）
