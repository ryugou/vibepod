# リリース前 E2E 検証チェックリスト

リリース前に手動で実行する E2E テスト手順。`cargo test` で検証できない Docker 実行・認証・コンテナ操作を対象とする。

## 前提条件

- Docker Desktop / OrbStack が起動している
- `vibepod init` 済み（Docker イメージがビルドされている）
- `vibepod login` 済み（認証トークンがある）
- テスト用の git リポジトリがある

## チェック項目

### 基本動作

- [ ] `vibepod --version` — バージョンが正しい
- [ ] `vibepod --help` — ヘルプが表示される
- [ ] `vibepod run --help` — run のオプション一覧が表示される

### インタラクティブモード

- [ ] `vibepod run` — 初回: コンテナ作成 → setup → Claude Code セッション開始
- [ ] `vibepod run` — 2回目: setup スキップ → 即座に Claude Code セッション開始（コンテナ再利用）
- [ ] コンテナ内でファイル読み書きができる（/workspace にプロジェクトがマウントされている）
- [ ] Ctrl+C でセッション終了（コンテナは保持される）

### コンテナ管理

- [ ] `vibepod stop <name>` — コンテナが停止する（削除されない）
- [ ] `vibepod stop --all` — 全 VibePod コンテナが停止する
- [ ] `vibepod run --new` — 既存コンテナを破棄して新規作成される
- [ ] `vibepod run --new`（running 時）— エラーメッセージが表示される

### prompt モード

- [ ] `vibepod run --prompt "CLAUDE.md を読んで"` — fire-and-forget で実行され、stream-json の整形表示が出る
- [ ] 実行完了後に Result が表示される
- [ ] コンテナが自動的に停止・削除される

### worktree

- [ ] `vibepod run --prompt "..." --worktree` — .worktrees/ 配下に隔離されたワークツリーが作成される
- [ ] 実行完了後に worktree のパスとブランチ名が表示される
- [ ] メインの作業ツリーに影響がない

### --lang

- [ ] `vibepod run --lang rust` — コンテナ内で `cargo --version` が使える
- [ ] `vibepod run --lang node` — コンテナ内で `node --version` が使える

### --mount

- [ ] `vibepod run --mount /path/to/file` — 指定ファイルが /mnt/ 配下に read-only でマウントされる

### --env-file

- [ ] `vibepod run --env-file .env` — 環境変数がコンテナ内に渡される

### セッション排他制御（v1.4.1）

- [ ] `vibepod run --prompt "..."` 実行中に別ターミナルで `vibepod run` → 「セッション実行中です (PID: ...)」エラー
- [ ] `vibepod run --prompt "..."` 実行中に別ターミナルで `vibepod run --prompt "..."` → 同上
- [ ] `vibepod run` (interactive) 実行中に `vibepod run --prompt "..."` → 「セッション実行中です」エラー
- [ ] プロセスを kill -9 後、次の `vibepod run --prompt` が stale ロックを自動検出して起動できる

### グローバル設定マウント（v1.4.1）

- [ ] `~/.claude/CLAUDE.md` が存在する場合、コンテナ内で `/home/vibepod/.claude/CLAUDE.md` が読める
- [ ] `~/.claude/skills/` が存在する場合、コンテナ内で `/home/vibepod/.claude/skills/` が読める
- [ ] `~/.claude/agents/` が存在する場合、コンテナ内で `/home/vibepod/.claude/agents/` が読める

### plugins/data の per-container rw ステージ（親 ro マウント内の子 rw マウント）

親 ro マウント（`~/.claude/plugins/`）の内側に子 rw マウント（`plugins/data/` ステージ）を
重ねる構成はコンテナランタイム依存で、壊れてもユニットテストでは検知できない。このうち
「ネストした ro/rw マウント自体が docker 上で正しく重なるか」は
`tests/container_integration_test.rs`（`cargo test -- --ignored` で実行、CI の
`docker-integration` ジョブが毎回実行する）で自動検証されるようになった。`$HOME` 経由の
コンテナ側パス（`/home/vibepod/.claude/plugins[/data]`）側だけでなく、
`installed_plugins.json` のホスト絶対パス解決用に追加される「ホスト絶対パス側」の
2本目のマウントエントリについても、`nested_mount_invariants_hold_for_absolute_host_path_entry`
が合成パス（`/opt/vibepod-test-hosthome`。実ホストの `$HOME` は使わない）を使って同じ
rw/ro 不変条件を検証しており、以下の項目のうち自動化済みのものにはその旨を注記している。
手動でしか確認できないのは「コンテナ新規作成直後にステージが空であること」と
「setup marker 欠落による作り直し後にステージが空であること」の2項目のみで、それぞれ
実際の `vibepod run` フローに依存するため引き続き手動で確認すること。

**用語の区別（重要）**: 「コンテナ側 mountpoint」と「マウント元（source）」は別物。
- コンテナ側 mountpoint = 親 ro マウントの**ホスト側ソース**の中にある `data/`
  サブディレクトリ（`~/.claude/plugins/data`）。ここが無いと、ro マウント済みの親の中に
  docker がマウントポイントを作れず `EROFS` でコンテナ作成自体が失敗する
  （`prepare_plugins_data_mount` の `create_dir_all(&host_data_dir)` がこれを防いでいる）。
- マウント元（source） = 子 rw マウントのホスト側ソース（`runtime_dir/plugins-data` 相当）。
  こちらは ro 制約を受けないため無くても docker が自動作成でき、起動可否には影響しない
  （`ensure_plugins_data_stage_dir` が保証しているのは `0700` 権限であって、存在自体は
  docker が代わりに保証してしまうため意味が異なる）。

#### CI で自動検証済み（`cargo test -- --ignored` / CI の `docker-integration` ジョブが毎回実行。手動確認は不要）

- `docker exec <container> touch /home/vibepod/.claude/plugins/data/.probe` が成功する（親 ro マウント内の子 rw マウントが機能している）（`container_integration_test.rs::nested_child_mount_is_writable`）
- `docker exec <container> touch <ホストの $HOME>/.claude/plugins/data/.probe` も成功する（`plugins_data_mount_entries` が返すもう一方のマウント先＝ホスト絶対パス側でも rw になっている）（`container_integration_test.rs::nested_mount_invariants_hold_for_absolute_host_path_entry`。合成パス `/opt/vibepod-test-hosthome` を `container_home` として渡すことで、`home == /home/vibepod` では生成されない2本目のエントリを再現している）
- 事前にホスト側 `~/.claude/plugins/data/` に一意な名前のファイル（例: `vibepod-host-sentinel-<日付>`）を作っておき、`docker exec <container> ls /home/vibepod/.claude/plugins/data` にそれが**現れない**（ホストの `~/.claude/plugins/data` の内容、他プロジェクトの codex job 履歴等が見えない）（`container_integration_test.rs::host_parent_content_is_hidden_by_child_mount`）
- `docker exec <container> touch /home/vibepod/.claude/plugins/data/vibepod-container-write-probe` の後、ホストの `~/.claude/plugins/data/` に `vibepod-container-write-probe` が**現れない**（コンテナ内の書き込みがホストへ汚染しない）（`container_integration_test.rs::container_writes_do_not_leak_to_host_parent`）
- `docker exec <container> touch /home/vibepod/.claude/plugins/.probe` が read-only で失敗する（親の ro が維持されている）（`container_integration_test.rs::parent_mount_stays_read_only`）
- 親 ro マウントのホスト側ソースに `data/` サブディレクトリが存在しない状態で `docker run` すると、コンテナ作成自体が失敗する（`container_integration_test.rs::missing_nested_mount_target_fails_container_creation`）。`prepare_plugins_data_mount` の `create_dir_all(&host_data_dir)` が無いとユーザーのコンテナが起動すらしなくなることの回帰検知

#### 手動で確認（実際の `vibepod run` フローに依存するため統合テストの対象外）

- [ ] コンテナを新規作成（`vibepod run --new`）した直後、`docker exec <container> ls -A /home/vibepod/.claude/plugins/data` が空である（per-container ステージは新規作成のたびに必ず空という不変条件の確認）（`reset_plugins_data_stage` の呼び出しタイミングは `vibepod run` の実フローに依存するため、`docker run` を直接叩く統合テストの対象外）
- [ ] `docker exec <container> rm /home/vibepod/.vibepod-setup-done` でセットアップ完了マーカーを削除してから `vibepod run` を実行すると、コンテナが削除・再作成され、その直後 `docker exec <container> ls -A /home/vibepod/.claude/plugins/data` が空である（setup marker 欠落による作り直し経路でも不変条件が保たれることの確認）（同上の理由）

### ps / logs

- [ ] `vibepod ps` — コンテナ一覧が表示される（CONTAINER / PROJECT / ELAPSED / LAST OUTPUT / STATUS の列が正しい）
- [ ] `vibepod ps` — `--prompt` 実行中のコンテナで ELAPSED と LAST OUTPUT が表示される
- [ ] `vibepod logs <container>` — コンテナのログが表示される

### 認証

- [ ] `vibepod login` — OAuth フローが完了し、トークンが保存される
- [ ] `vibepod logout` — トークンが削除される
- [ ] トークン期限切れ時に `vibepod run` が適切なエラーメッセージを出す

### restore

- [ ] `vibepod restore` — セッション一覧が表示され、選択したセッションの状態に復元できる

### エラーケース

- [ ] Docker 未起動時に `vibepod run` → 適切なエラーメッセージ
- [ ] 未認証時に `vibepod run` → 「vibepod login を実行してください」
- [ ] git リポジトリ外で `vibepod run` → 適切なエラーメッセージ
- [ ] 存在しないイメージで `vibepod run` → 「vibepod init を実行してください」
