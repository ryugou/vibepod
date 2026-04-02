# レビュー指摘対応 + リファクタリング仕様

## 概要

コードレビューで受けた指摘（目的のギャップ、セキュリティ、v2 リファクタリング、コード品質、ドキュメント乖離）への対応。

## Phase 1: bridge 削除 + Docker 実行経路統一

### 1-A: bridge モジュール削除

bridge モジュールを完全に削除する。v2 の daemon 化で代替されるため不要。

**削除対象:**
- `src/bridge/` ディレクトリ全体（mod.rs, io.rs, slack.rs, logger.rs, detector.rs, formatter.rs）
- `tests/bridge_logger_test.rs`
- `tests/bridge_slack_test.rs`
- `tests/bridge_formatter_test.rs`

**変更対象:**
- `src/lib.rs` — `pub mod bridge;` を削除
- `src/cli/mod.rs` — Run サブコマンドから以下のオプションを削除:
  - `--bridge`
  - `--notify-delay`
  - `--slack-channel`
  - `--llm-provider`
- `src/cli/run.rs`:
  - `RunOptions` から `bridge`, `notify_delay`, `slack_channel`, `llm_provider` フィールドを削除
  - `run_bridge` 関数を完全削除
  - `execute` 関数から bridge 分岐を削除
  - bridge.env 読み込みロジック（run_bridge 内）は関数ごと消えるので自動的に削除
- `src/main.rs` — Commands::Run から bridge 関連フィールドの受け渡しを削除
- `Cargo.toml` — 以下のクレートが bridge 以外で使われていなければ削除:
  - `tokio-tungstenite`（bridge 専用、削除）
  - `strip-ansi-escapes`（bridge 専用、削除）
  - `regex` — `src/` 内で bridge 以外に使っているか `grep -r "regex" src/ --include="*.rs" | grep -v bridge/` で確認して判断
  - `chrono` — 同上で確認
  - `libc` — 同上で確認

### 1-B: Docker 実行経路を docker run に統一

現在、インタラクティブモードは `Command::new("docker").args(["run", "-it", ...])` で実行し、prompt/resume モードは bollard API（`create_and_start_container` + `stream_logs_formatted`）で実行している。これを docker run に統一する。

**run_fire_and_forget の書き換え:**

現在の bollard フロー:
```
create_and_start_container → stream_logs_formatted → stop_container → remove_container
```

新しい docker run フロー:
```
docker run -d --name {name} ... → docker logs --follow {name} (stdout パイプで format_stream_event) → docker stop {name} → docker rm {name}
```

具体的な実装:
1. `ContainerConfig` から docker run の引数リストを生成するメソッド `to_docker_args(&self) -> Vec<String>` を追加
2. `run_fire_and_forget` を書き換え:
   - `docker run -d --rm` ではなく `docker run -d`（--rm だと logs が途中で切れる可能性）でコンテナ起動
   - `Command::new("docker").args(["logs", "--follow", &container_name])` で stdout をパイプ
   - stdout を `BufReader::new(child.stdout)` で 1 行ずつ読み、`format_stream_event` でパース・整形表示
   - `result` イベントの値を保持
   - `tokio::select!` で Ctrl+C 割り込み対応
   - 終了後に `docker stop` + `docker rm -f`
3. `run_interactive` も `ContainerConfig::to_docker_args` を使って docker_args の直接組み立てを廃止

**docker.rs の書き換え:**

bollard 依存を完全に外し、docker CLI ラッパーに変更する。

残す関数（docker CLI ラッパーに書き換え）:
- `image_exists` — `docker inspect {image}` の exit code で判定。エラー種別を区別し、image not found のみ false、それ以外は Error 返却
- `find_running_container` — `docker ps --filter "name={prefix}" --format "{{.ID}} {{.Names}}"` で検索
- `list_vibepod_containers` — `docker ps -a --filter "name=vibepod-" --format "{{.Names}} {{.Status}}"`
- `find_container_by_name` — `docker ps -a --filter "name={name}" --format "{{.ID}}"`
- `get_logs` — `docker logs --tail {tail} {container_id}`
- `build_image` — `docker build` コマンド（既存の tar ビルドロジックは Docker CLI に任せる。Dockerfile を一時ファイルに書き出して `docker build -f` で実行）
- `ping` — `docker info` の exit code で判定

削除する関数:
- `create_and_start_container` — run_fire_and_forget が docker run に変わるため不要
- `stream_logs` / `stream_logs_formatted` — docker logs コマンドに置き換え
- `stop_container` / `remove_container` — docker stop/rm コマンドに置き換え（run.rs 内で直接呼ぶ）
- `wait_container` — docker wait コマンドに置き換え
- `attach_container` — bridge 削除で不要
- `resize_container_tty` — bridge 削除で不要

**DockerRuntime 構造体:**
- `docker: Docker` フィールドを削除（bollard の型）
- 必要に応じてシンプルなマーカー構造体にするか、関連関数群にする

**Cargo.toml:**
- `bollard` を削除
- `futures-util` — bollard の StreamExt で使用していた。他で使っていなければ削除
- `tar` — build_image の tar ビルドで使用。docker build -f に変えるなら削除可能

**ContainerConfig の統一:**
- `ContainerConfig` に `to_docker_args(&self, interactive: bool) -> Vec<String>` メソッドを追加
- interactive=true の場合は `-it --rm` を付与
- interactive=false の場合は `-d` を付与
- マウント、env、setup_cmd、ネットワーク設定を共通で組み立て
- `run_interactive` と `run_fire_and_forget` の両方がこのメソッドを使う

**format_stream_event の移動:**
- `src/runtime/stream.rs` を新規作成
- `format_stream_event`, `StreamEvent` を docker.rs から stream.rs に移動
- `src/runtime/mod.rs` に `pub mod stream;` を追加

### Phase 1 完了条件
- `cargo fmt && cargo clippy` が通る
- `cargo test` が通る（bridge 関連テスト削除済み）
- `cargo build --release` が成功
- `./target/release/vibepod run --help` に `--bridge` 関連オプションがない
- `./target/release/vibepod --help` が正常動作

### Phase 1 コミット
- codex review を実行（`codex review -c sandbox_mode=danger-full-access -c approval_policy=never`、timeout: 600000）
- 指摘がなくなるまで修正（最大 5 回）
- Conventional Commits 準拠でコミット: `refactor: remove bridge module and unify docker execution to docker run`

---

## Phase 2: run.rs 分割 + 設定統合 + セッション永続化 + コード品質

### 2-A: run.rs をファイル分割

`src/cli/run.rs` を `src/cli/run/` ディレクトリに分割する。

**分割方法:**
1. `src/cli/run.rs` を `src/cli/run/mod.rs` にリネーム
2. 以下の関数・構造体を個別ファイルに移動:

`src/cli/run/mod.rs`（残すもの）:
- `RunOptions` 構造体
- `RunContext` 構造体
- `execute` 関数
- `build_container_config` 関数
- 定数 `VALID_REVIEWERS`
- 公開ユーティリティ関数: `parse_mount_arg`, `detect_languages`, `get_lang_install_cmd`, `validate_slack_channel_id`, `resolve_reviewers`, `build_review_prompt`

`src/cli/run/prepare.rs`:
- `prepare_context` 関数

`src/cli/run/interactive.rs`:
- `run_interactive` 関数

`src/cli/run/prompt.rs`:
- `run_fire_and_forget` 関数

各ファイルから mod.rs の型や関数を `use super::*` または明示的な import で参照する。

### 2-B: 設定の統合

config.json のプロジェクト登録を config.toml に移行する。

**変更内容:**

`src/config/global.rs`:
- `GlobalConfig` のシリアライズ/デシリアライズを JSON → TOML に変更
- 保存先: `~/.config/vibepod/config.toml` の `[global]` セクション
- マイグレーション: `config.json` が存在し `config.toml` に `[global]` がなければ自動変換

`src/config/projects.rs`:
- プロジェクト一覧のシリアライズ/デシリアライズを JSON → TOML に変更
- 保存先: `~/.config/vibepod/config.toml` の `[[projects]]` セクション
- マイグレーション: `projects.json` が存在し `config.toml` に `[[projects]]` がなければ自動変換

統合後の `~/.config/vibepod/config.toml` の形式:
```toml
[global]
default_agent = "claude"
image = "vibepod-claude:latest"

[[projects]]
name = "vibepod"
path = "/Users/user/src/vibepod"
remote = "github.com/user/vibepod"
registered_at = "2026-04-01T00:00:00Z"

[review]
reviewers = ["codex", "copilot"]

[run]
lang = "rust"
```

`Cargo.toml`:
- `toml` クレートを追加（TOML パース/シリアライズ用）。既に `toml` が入っているか確認して、なければ追加。

### 2-C: セッション永続化のディレクトリ構造統一

セッション関連データの保存先を `.vibepod/sessions/{session_id}/` に統一する。

**変更内容:**

`src/session.rs`:
- 保存先を `.vibepod/sessions/{session_id}/metadata.json` に変更
- 一覧取得は `.vibepod/sessions/` ディレクトリを列挙
- 既存の `.vibepod/sessions.json`（もしあれば）からのマイグレーション

`src/report.rs`:
- 保存先を `.vibepod/sessions/{session_id}/report.md` に変更

### 2-D: コード品質改善

1. `format_stream_event` + `StreamEvent` の `src/runtime/stream.rs` への分離（Phase 1 で実施済みならスキップ）
2. `image_exists` のエラーハンドリング改善（Phase 1 で実施済みならスキップ）
3. 主要な公開構造体に短い rustdoc を追加:
   - `RunOptions` — CLI run サブコマンドのオプション
   - `DockerRuntime` — Docker CLI ラッパー
   - `ContainerConfig` — コンテナ起動設定
   - `VibepodConfig` — プロジェクト + グローバル設定のマージ結果
   - `GlobalConfig` — グローバル設定
   - `SessionStore` — セッション履歴管理
   - `AuthManager` — OAuth トークン管理

### Phase 2 完了条件
- `cargo fmt && cargo clippy` が通る
- `cargo test` が通る
- `cargo build --release` が成功
- run.rs が分割されている（`src/cli/run/` ディレクトリに mod.rs, prepare.rs, interactive.rs, prompt.rs）
- `./target/release/vibepod run --help` が正常動作

### Phase 2 コミット
- codex review を実行
- 指摘がなくなるまで修正（最大 5 回）
- Conventional Commits 準拠でコミット: `refactor: split run.rs, unify config to toml, restructure session storage`

---

## Phase 3: ドキュメント更新

### README.md

1. コマンド一覧に `vibepod ps`, `vibepod logs` を追加
2. `--bridge`, `--notify-delay`, `--slack-channel`, `--llm-provider` 関連を削除
3. Roadmap に v1.5 を追加:
   - v1.5: `vibepod ps`, `vibepod logs`, `--mount`, `vibepod.toml` config, `--review` 進化（Codex + Copilot）、bridge 削除、docker run 統一、run.rs 分割
4. セキュリティ 3 層の説明を更新:
   - .gitconfig の read-only マウントを明記
   - GH_TOKEN の自動注入を明記
   - --mount による追加マウント（read-only）を明記
   - ~/.claude.json は一時コピー経由でマウント（ホスト直書き防止）を明記
5. インタラクティブ vs --prompt のセキュリティモデルの違いを追記:
   - インタラクティブ: `--dangerously-skip-permissions` なし（ユーザーが承認操作）
   - --prompt: `--dangerously-skip-permissions` あり（自動実行、コンテナ隔離が安全境界）

### SECURITY.md

1. GH_TOKEN 自動注入のリスクを追記:
   - `gh auth token` でホスト側の GitHub トークンを自動取得してコンテナに注入
   - コンテナ内プロセスは GitHub に対して強い権限を持つ可能性がある
2. `op run --no-masking` のリスクを追記:
   - 解決済みシークレットがサブプロセスの stdout に出る経路
   - ログ共有環境での注意
3. `--mount` の信頼境界を追記:
   - ユーザー指定のホストパスを read-only でマウント
   - 信頼境界は「VibePod を起動したユーザー」
   - パストラバーサルや意図しないファイル露出は設定ミスで起こりうる
4. Claude のネットワーク利用について補足:
   - 「Standard mode は外部送信なし」を「Claude API へのネットワーク通信はコンテナ内の通常動作」と明確化
   - 「完全オフライン」ではないことを明記
5. bridge 関連のセクション（Data Transmission の Bridge mode、Trust Model の Slack channel security）を削除
6. `vibepod login` の一時コンテナが `--network host` を使用する点を追記

### docs/design.md

1. マウント表（220-225 行付近）を更新:
   - `~/.claude.json` を「一時ファイル経由で RW マウント（ホスト直書き防止）」に変更
   - `~/.gitconfig` を read-only マウントとして追加（既にあれば確認）
   - `--mount` による追加マウント（read-only）を追加
   - Codex 使用時の `~/.codex/auth.json` read-only マウントを追加
   - `GH_TOKEN` 環境変数の自動注入を追加
2. CLI 出力イメージ（280 行付近）を更新:
   - `Mode: --dangerously-skip-permissions` → `Mode: interactive`（インタラクティブがデフォルト）に変更
3. Dockerfile スニペット（113-146 行付近）を更新:
   - `gh` CLI のインストールを追加
4. プロジェクト構成（44-80 行付近）を更新:
   - `src/cli/run/` ディレクトリ構造（mod.rs, prepare.rs, interactive.rs, prompt.rs）
   - `src/cli/ps.rs`, `src/cli/logs.rs` を追加
   - `src/config/vibepod_config.rs` を追加
   - `src/bridge/` を削除
   - `src/runtime/stream.rs` を追加
5. vp エイリアス（376-378 行付近）:
   - 「同一バイナリを指す」を「インストーラがシンボリックリンクを作る」に変更

### Phase 3 完了条件
- ドキュメントの内容が実装と一致している
- bridge 関連の記述がすべて削除されている

### Phase 3 コミット
- codex review を実行
- 指摘がなくなるまで修正（最大 5 回）
- Conventional Commits 準拠でコミット: `docs: update README, SECURITY.md, and design.md to match v1.5 implementation`

---

## 最終: PR 作成

全 Phase のコミットが完了したら:
1. `git push -u origin refactor/review-findings-v2`
2. `gh pr create --base main --title "refactor: address review findings - bridge removal, docker run unification, docs update"`

PR の body には各 Phase の変更概要を含める。
