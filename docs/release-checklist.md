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
