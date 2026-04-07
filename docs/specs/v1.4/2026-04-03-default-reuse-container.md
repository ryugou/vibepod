# デフォルトコンテナ再利用

## 概要

コンテナの再利用をデフォルト動作にする。プロジェクトごとに 1 つのコンテナが存在し、`vibepod run` は既存コンテナに `docker exec` で接続する。初回のみコンテナを作成して setup を実行する。

## 設計方針

- **プロジェクト = コンテナ 1:1**。プロジェクトディレクトリのパスから一意なコンテナ名を生成
- **デフォルトは再利用**。`--reuse` フラグは廃止
- **`--new` フラグ**で明示的に新規作成。既存コンテナが running ならエラー（「先に vibepod rm してください」）
- **setup は初回のみ**。マーカーファイル `/.vibepod-setup-done` で完了判定

## コンテナ名

`vibepod-{project_name}-{path_hash_8chars}`

- `project_name`: ディレクトリ名（例: `my-app`）
- `path_hash_8chars`: ディレクトリの絶対パスの SHA256 先頭 8 文字（hex）

例: `/Users/user/work/my-app` → `vibepod-my-app-a1b2c3d4`

## コンテナライフサイクル

### 初回実行（コンテナなし）

1. `docker run -d --name {name} ... tail -f /dev/null` でコンテナ作成（エントリポイントは idle のみ）
2. setup_cmd がある場合: `docker exec {name} sh -c "{setup_cmd} && touch /.vibepod-setup-done"` で setup 実行
3. setup_cmd がない場合: `docker exec {name} touch /.vibepod-setup-done` でマーカーのみ作成
4. setup 失敗（exit code != 0）→ コンテナを自動 rm → エラー返却
5. `docker exec [-it] {name} claude {args}` で Claude 実行

### 2 回目以降（コンテナあり: stopped）

1. `docker start {name}` でコンテナ再開
2. `docker exec {name} cat /.vibepod-setup-done` でマーカー確認
3. マーカーなし → setup が未完了 → コンテナを rm して初回フローに戻る
4. マーカーあり → `docker exec [-it] {name} claude {args}` で Claude 実行

### 2 回目以降（コンテナあり: running）

1. そのまま `docker exec [-it] {name} claude {args}` で Claude 実行（並行 exec）

### `--new` 指定時

1. 同名コンテナが running → エラー: 「Container is running. Stop it with `vibepod stop` or `vibepod rm` first.」
2. 同名コンテナが stopped → 自動 rm → 初回フローに戻る
3. コンテナなし → 初回フローに戻る

## 認証トークンの扱い

`CLAUDE_CODE_OAUTH_TOKEN` と `GH_TOKEN` はコンテナ作成時の env ではなく、毎回 `docker exec -e` で最新の値を渡す。

```
docker exec -e CLAUDE_CODE_OAUTH_TOKEN={token} -e GH_TOKEN={gh_token} {name} claude {args}
```

インタラクティブの場合:
```
docker exec -it -e CLAUDE_CODE_OAUTH_TOKEN={token} -e GH_TOKEN={gh_token} {name} claude {args}
```

## 設定変更の検知

コンテナ作成時の設定を Docker ラベルに保存し、次回実行時に比較する。差分があれば警告して `--new` を促す。

比較対象:
- `--mount` の値（全マウントパスをソートして結合）
- `--no-network` の値
- `--lang` の値
- `--review` による暗黙マウント（codex auth 等）

ラベル形式:
```
docker run --label vibepod.mounts="..." --label vibepod.network="..." --label vibepod.lang="..." ...
```

差分検出時のメッセージ:
```
Warning: Container configuration has changed (lang: rust → node).
Run with --new to recreate the container.
Continuing with existing container...
```

警告を出すが、そのまま続行する（C 方式）。

## `--worktree` の扱い

`--worktree` は毎回新しい workspace を作るため、常に使い捨てコンテナ（`--new` 扱い）で実行する。コンテナ名はランダムハッシュ（従来通り）。

## `vibepod stop` コマンド

```
vibepod stop [name]
vibepod stop --all
```

`docker stop` を呼ぶだけ。`vibepod rm` と異なりコンテナは削除しない。

## `vibepod init` の変更

イメージ再ビルド後に既存コンテナを全削除する。running コンテナがある場合は確認プロンプトを表示（非インタラクティブ時は強制削除）。

## `vibepod ps` の表示

PROJECT 列はディレクトリ名を表示。同名プロジェクトがあればパスを含めて表示（`...work/my-app`）。

## `--prompt` モードのログ保存

`--prompt` 実行時に stdout を `.vibepod/sessions/{session_id}/logs.txt` に同時書き出しする。

## `--reuse` フラグの廃止

`--reuse` を削除し、デフォルトで再利用する。`--new` で新規作成。

## 変更対象ファイル

### CLI
- `src/cli/mod.rs` — `--reuse` 削除、`--new` 追加、`stop` サブコマンド追加
- `src/main.rs` — `--new` の受け渡し、`stop` のルーティング
- `src/cli/stop.rs` — 新規作成

### 実行ロジック
- `src/cli/run/mod.rs` — `RunOptions` から `reuse` 削除、`new_container` 追加
- `src/cli/run/prepare.rs` — コンテナ名をパスハッシュベースに変更、設定ラベルの保存・比較ロジック、マーカーチェック
- `src/cli/run/interactive.rs` — デフォルト再利用フロー（exec ベース）、`docker exec -e` でトークン渡し
- `src/cli/run/prompt.rs` — デフォルト再利用フロー（exec ベース）、`docker exec -e` でトークン渡し、ログファイル書き出し

### ランタイム
- `src/runtime/docker.rs` — ラベル関連メソッド追加（`get_container_labels`、`create_container_with_labels` 等）、マーカーチェックメソッド

### 初期化
- `src/cli/init.rs` — ビルド後に全コンテナ削除（確認プロンプト付き）

### ドキュメント
- `README.md` — `--reuse` → `--new` の変更、`vibepod stop` 追加、デフォルト動作の説明更新
- `docs/design.md` — コンテナライフサイクルの説明更新

## 完了条件

- `cargo fmt && cargo clippy` が通る
- `cargo test` が通る
- `cargo build --release` が成功
- `./target/release/vibepod run --help` に `--new` があり `--reuse` がない
- `./target/release/vibepod stop --help` が動作する

## コミット

- codex review を実行（`codex review -c sandbox_mode=danger-full-access -c approval_policy=never`、timeout: 600000）
- 指摘がなくなるまで修正（最大 5 回）
- Conventional Commits 準拠でコミット
- `git push -u origin feat/default-reuse-container`
- `gh pr create --base main`
