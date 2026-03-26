# VibePod Phase 0: Slack Bridge — Design Spec

## Overview

VibePod の `run` コマンドに `--bridge` フラグを追加し、コンテナ出力の無音検知 → Slack 通知 → Slack/ターミナルからの応答 → コンテナ stdin 送信 を実現する。ターミナルでの透過表示を維持しつつ、離席時は Slack がバックアップとなるハイブリッド型。

`--bridge` は interactive モード（引数なし起動）でも fire-and-forget モード（`--prompt`）でも使用可能。どちらでも Claude Code が確認待ちになる可能性があり、またタスク完了通知としても有用。

Phase 0 の目的は、入力待ちパターンの知見蓄積と離席時の簡易応答の実用化。

**対象プラットフォーム**: macOS / Linux（Unix 前提）。Windows は対象外。

## Architecture

```
vibepod run --bridge
│
├── DockerRuntime (既存)
│     └── コンテナ起動（tty=true, open_stdin=true, attach_stdin=true）
│
├── Bridge (新規: src/bridge/)
│     ├── bollard attach_container で stdin/stdout ストリーム取得
│     ├── ホスト側ターミナルを raw mode に設定
│     └── バイト列を透過転送（ANSI エスケープ含む完全透過）
│
├── tokio runtime
│     ├── container_read  : attach stdout → ターミナル stdout 転送 + バッファ蓄積
│     ├── terminal_write  : ターミナル stdin → attach stdin 転送
│     ├── idle_watch      : 無音 N 秒検知 → Slack 通知トリガー
│     ├── slack_out       : Slack Socket Mode → 通知送信
│     ├── slack_in        : Slack 応答受信 → attach stdin 書き込み
│     ├── resize          : SIGWINCH → resize_container_tty API（Unix のみ）
│     └── signal          : Ctrl+C / コンテナ終了検知
│
└── --bridge なしの場合
      └── 既存の docker run -it / bollard stream_logs パス（変更なし）
```

### Design Decisions

- **VibePod サブコマンドとして統合** — 別バイナリや別クレートにせず、`vibepod run --bridge` として実装。コード共有が容易。
- **bridge 時は常に bollard でコンテナ作成〜attach まで一貫** — interactive モードであっても `docker` CLI サブプロセスは使わず、bollard API でコンテナ作成・起動・attach を行う。DockerRuntime の `create_and_start_container` を経由し、attach_container でストリームを取得する。これにより DockerRuntime の責務境界が明確になる。
- **シングルプロセス・マルチタスク** — tokio タスクで読み書き、無音検知、Slack 通信を並行処理。IPC 不要でシンプル。
- **全モード対応** — `--bridge` は interactive / `--prompt` / `--resume` のいずれでも使用可能。
- **後付け bridge attach は Phase 0 スコープ外** — daemon 化（v2）で対応予定。docs/proposal-v2-dashboard.md に将来課題として記録済み。

## Container I/O Management

```
┌─ VibePod プロセス ───────────────────┐
│                                      │
│  bollard attach_container            │
│    stdout stream ←── コンテナ pty     │
│    stdin stream  ──→ コンテナ pty     │
│      │                               │
│      ├── read → ターミナル stdout     │
│      ├── read → バッファ蓄積          │
│      └── write ← ターミナル stdin     │
│               ← Slack 応答           │
│                                      │
│  ターミナル: raw mode                 │
│  リサイズ: SIGWINCH → bollard         │
│            resize_container_tty()    │
└──────────────────────────────────────┘
```

- 既存の `DockerRuntime` でコンテナを起動（`tty: true`, `open_stdin: true`, `attach_stdin: true`）
- bollard の `attach_container` で stdin/stdout の非同期ストリームを取得
- ホスト側ターミナルを raw mode に設定し、バイト列をそのまま透過転送（ANSI 完全透過）
- 読み取ったバイト列は stdout への書き出しと同時にバッファにもコピー
- ターミナル stdin からの入力と Slack からの応答は同じ attach stdin に書き込む（tokio mpsc channel 経由）
- `SIGWINCH` シグナルを監視し、bollard の `resize_container_tty` API でコンテナ側 pty のウィンドウサイズを同期（Unix のみ）

### Existing Code Integration

- `src/cli/run.rs` の `execute()` 内で `--bridge` 判定（セッション記録の後、Docker 起動の前）
- bridge あり → `bridge::run()` に委譲（Docker 起動引数の構築は既存コードと共有）
  - interactive モードでも bollard attach を使用（既存の `docker` CLI サブプロセスパスとは異なる実行パス）
- bridge なし → 既存の実行パス（変更なし）

### Session Recording

セッション記録（`.vibepod/sessions.json` への `SessionStore::add()`）は `--bridge` の分岐前に行う。既存の `run::execute()` 冒頭のタイミングを維持し、bridge あり/なしで記録ロジックを共有する。bridge::run() 内では記録しない（二重記録の防止）。

## Idle Detection & Notification Flow

### State Machine

```
[Buffering]
  │ コンテナ出力あり → バッファ蓄積 + タイマーリセット（Buffering に留まる）
  │ ターミナル入力あり → バッファクリア + タイマーリセット（Buffering に留まる）
  │
  │ N秒間出力なし かつ ターミナル入力なし
  ▼
[Idle]
  │ バッファ内容を取得（最大40行 / 2500文字）
  │ Slack に通知送信
  ▼
[WaitingResponse]
  │ ターミナル stdin → コンテナに転送、Slack 通知を「応答済み」に更新
  │ Slack ボタン/リアクション/スレッド返信 → コンテナに転送、同上
  │ コンテナ出力再開 → 応答なしでも Buffering に遷移（新たな出力を蓄積開始）
  │
  │ 先着1つを採用（AtomicBool ガードで即ロック）
  ▼
[Buffering]（バッファクリア済み、次の無音検知へ）
```

### States

- **Buffering** — 出力を蓄積中。出力がある度にタイマーリセット。ターミナル入力があってもタイマーリセット（通知キャンセル）。
- **Idle** — N秒無音かつターミナル入力なし。Slack 通知を送信し、即座に WaitingResponse に遷移。
- **WaitingResponse** — 通知済み、応答待ち。ターミナル or Slack どちらからでも応答可能。コンテナ出力が再開した場合は応答の有無にかかわらず Buffering に遷移する。

### Response Deduplication

先着1件の応答を採用する。具体的な挙動：

- 応答受信時に `AtomicBool` を即座に `true` にセット。以降の応答は stdin に書き込まず黙って破棄する（ログにも記録しない）。
- Slack 側のボタン無効化（メッセージ更新）は best-effort。ネットワーク遅延により更新前にボタンが押される可能性があるが、AtomicBool ガードにより二重送信は発生しない。
- ターミナル入力は「応答」として扱うが、ユーザーがターミナルにいる場合は連続入力が自然なため、ターミナル入力による応答後はガードをリセットし、後続のターミナル入力も通す。Slack 応答のみをブロックする。

### No Distinction Between Input-Waiting and Task Completion

入力待ちとタスク完了を区別しない。どちらも「出力が止まった。最後の状態はこれです」という統一通知。ユーザーが文脈で判断し、必要なら応答、不要なら無視する。

### Output Buffer

- 無音期間で区切る（前回の無音から今回の無音までが1ブロック）
- 上限: **40行 かつ 2,500文字**（いずれか先に達した方で切り詰め。Slack Block Kit の 3,000 文字制限に対する安全マージン）
- 超過時は末尾を残し、先頭に `... (N lines truncated)` を表示
- Slack 送信時は ANSI エスケープシーケンスをストリップする（`strip-ansi-escapes` クレート使用）。ストリップ後の文字数で上限を判定。
- バッファクリアのタイミング: 応答受信後に即クリア。WaitingResponse 中にコンテナ出力が再開した場合もクリアして新規蓄積を開始。

## Slack Communication

### Connection

Socket Mode（WebSocket）で接続。HTTP エンドポイント不要。

接続断時は exponential backoff（初回1秒、最大60秒、最大試行5回）で再接続を試みる。5回失敗した場合は stderr に警告を出し、ターミナル専用モードにフォールバック（bridge なしと同等の動作を継続）。

### Notification Message Format

```
🔔 VibePod: セッション出力が停止しました

┌──────────────────────────────┐
│ ```                          │
│ (直近の出力、最大40行)         │
│ ```                          │
├──────────────────────────────┤
│ [Yes]  [No]  [Skip]         │
└──────────────────────────────┘

→ スレッドに返信でテキスト入力も可能
```

複数プロジェクトが同時に bridge を使用する場合の混線防止のため、通知メッセージにセッション ID とプロジェクト名を含める：
```
🔔 VibePod [my-project] (session: 20260325-143000-a1b2)
```

### Response Mapping

| Method | Action | stdin |
|--------|--------|-------|
| Button | Yes | `y\n` |
| Button | No | `n\n` |
| Button | Skip | `\n` |
| Reaction | 👍 | `y\n` |
| Reaction | 👎 | `n\n` |
| Reaction | ⏭️ | `\n` |
| Thread reply | text | `{text}\n` |

### Post-Response

応答後、元メッセージのボタンを無効化し応答内容を表示：
```
✅ 応答済み: "y" (ボタン: Yes)
✅ 応答済み: "fix the tests please" (スレッド返信)
```

### Session Lifecycle Messages

- Start: `🟢 VibePod セッション開始 [project-name] (session: ID)`
- End: `🔴 VibePod セッション終了 [project-name] (exit code: N)`

exit code はコンテナの終了コード（bollard `wait_container` API で取得）。

### Slack Crate Selection

`slack-morphism` のメンテナンス状況を実装時に確認。判断基準：
- crates.io での最終リリースが6ヶ月以内か
- 現行の Slack API バージョンに対応しているか

メンテされていれば採用、されていなければ `reqwest` + `tokio-tungstenite` で Slack Web API / Socket Mode を薄くラップする自前実装。自前実装の場合、Phase 0 で必要な最小サブセットのみ実装する：
- Socket Mode: WebSocket 接続、hello ハンドシェイク、envelope ack
- Web API: `chat.postMessage`, `chat.update`（通知送信・更新のみ）
- Events: `interaction`（ボタン）、`reaction_added`、`message`（スレッド返信）

不要な機能（チャンネル管理、ユーザー管理、ファイルアップロード等）は実装しない。

### Slack App Scopes — Implementation Checklist

Bot Token Scopes は実装時に Slack API ドキュメントで最終確認する。想定される必要スコープ：
- `chat:write` — メッセージ投稿・更新
- `reactions:read` — リアクション検知

`channels:history` が実際に必要かは実装時に検証する（Event Subscriptions の `message` イベントでスレッド返信を受信できれば不要な可能性がある）。

## Logging

知見蓄積のため、最小限のイベントを JSON Lines で記録。

### Output Path

`{vibepod_config_dir}/bridge-logs/{session-id}.jsonl`

（`vibepod_config_dir` は既存の `config::default_config_dir()` = `dirs::config_dir().join("vibepod")`。macOS: `~/Library/Application Support/vibepod/bridge-logs/`。既存の `config.json`, `projects.json`, `auth/` と同階層。）

### Events

```jsonl
{"ts":"2026-03-25T14:35:30+09:00","event":"notified","last_lines":"Do you want to proceed? (y/n)"}
{"ts":"2026-03-25T14:36:05+09:00","event":"responded","source":"slack_button","stdin_sent":"y\n","response_time_seconds":35}
```

- `notified` — Slack 通知送信時。`last_lines` に表示した出力を記録
- `responded` — 応答受信時。`source`（terminal / slack_button / slack_reaction / slack_thread）、送信内容、応答時間を記録
  - ターミナル応答時の `stdin_sent` は `"(terminal input)"` 固定（機密情報混入防止のため生テキストは記録しない）

### Privacy

`last_lines` や `stdin_sent` に API キー・パスワード等の機密情報が混入する可能性がある。Phase 0 ではローカルファイルへの記録のみ（外部送信なし）のため、ファイル権限を 0600 に設定して対応する。ログの外部共有や分析ツールへの投入時はユーザー自身が確認する前提。将来的にマスク機能（正規表現ベースのフィルタ）が必要になった場合は追加検討する。

## CLI Interface

### New Options on `vibepod run`

```
--bridge              Slack Bridge モードを有効化
--notify-delay <SEC>  無音検知から通知までの秒数（デフォルト: 30）
--slack-channel <ID>  通知先チャンネルを上書き
```

`--bridge` は既存のオプション（`--prompt`, `--resume`, `--no-network`, `--env`, `--env-file`）と併用可能。

### Usage Example

```bash
vibepod run --bridge
vibepod run --bridge --notify-delay 10
vibepod run --bridge --slack-channel C0AJHQRE23Z
vibepod run --bridge --prompt "fix the login bug"
vibepod run --bridge --env-file ./app.env   # --env-file はコンテナ用（bridge.env とは独立）
```

### Configuration

Bridge 用の環境変数はデフォルトで `{vibepod_config_dir}/bridge.env` から読み込む。`op://` 参照にも対応（既存の 1Password CLI 連携を流用）。

```bash
# {vibepod_config_dir}/bridge.env
SLACK_BOT_TOKEN="op://ai-agents/slack-bridge/bot-token"
SLACK_APP_TOKEN="op://ai-agents/slack-bridge/app-token"
SLACK_CHANNEL_ID=C0AJHQRE23Z
```

Slack トークン（`SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`）は常に `bridge.env` から読み込む。`--env-file` はコンテナ用環境変数専用で、bridge 設定とは独立。

チャンネル ID の優先順位:
1. `--slack-channel` CLI オプション（最優先）
2. `{vibepod_config_dir}/bridge.env` 内の `SLACK_CHANNEL_ID`

### File Layout

既存の設定ディレクトリ構造との関係：

```
{vibepod_config_dir}/          # config::default_config_dir()
├── config.json                # 既存: グローバル設定
├── projects.json              # 既存: プロジェクト登録
├── auth/
│   └── token.json             # 既存: OAuth トークン
├── bridge.env                 # 新規: Slack Bridge 環境変数
└── bridge-logs/               # 新規: Bridge ログ
    └── {session-id}.jsonl
```

### Validation

`--bridge` 指定時、起動前に以下をチェック：
- `SLACK_BOT_TOKEN` と `SLACK_APP_TOKEN` が解決できること
- `SLACK_CHANNEL_ID` がいずれかの方法で指定されていること
- 不足時はエラーメッセージで何が足りないか明示して終了

## Module Structure

```
src/
├── bridge/
│   ├── mod.rs        # pub fn run() — bridge モードのエントリポイント
│   ├── io.rs         # bollard attach、ターミナル raw mode（RAII ガード + panic hook で復元保証）、stdin/stdout 転送、リサイズ
│   ├── detector.rs   # 無音検知、バッファ管理、状態遷移（Buffering/Idle/WaitingResponse）、ANSI ストリップ
│   ├── slack.rs      # Socket Mode 接続、通知送信、応答受信、再接続（exponential backoff）
│   └── logger.rs     # JSON Lines ログ記録
├── cli/
│   ├── mod.rs        # --bridge, --notify-delay, --slack-channel 追加
│   └── run.rs        # --bridge 判定で bridge::run() に分岐（セッション記録後）
└── ...               # 既存モジュール（変更なし）
```

モジュール間は tokio の channel (`mpsc`) で疎結合に接続：
```
io.rs → (出力バイト列) → detector.rs → (通知トリガー) → slack.rs
slack.rs → (応答テキスト) → io.rs
```

## Additional Dependencies

```toml
# Cargo.toml に追加
strip-ansi-escapes = "0.2"    # Slack 送信時の ANSI ストリップ

# tokio features 追加: "io-util", "time", "sync"
# 実装時に "io-std", "net" の要否も確認し、必要なら追加（"full" にはしない）
```

Slack クレートは実装時に選定（slack-morphism or reqwest + tokio-tungstenite）。

## Slack App Setup Requirements

- Bot Token Scopes: `chat:write`, `reactions:read`（`channels:history` の要否は実装時に検証）
- Socket Mode: 有効化
- Interactivity: ON
- Event Subscriptions: `reaction_added`, `message`

## Out of Scope (v2+)

- 後付け bridge attach（daemon 化で対応）
- pty 出力の全文ストリーミング（Dashboard）
- 複数セッションの並行管理（daemon）
- APNs プッシュ通知（Mobile App）
- セッションの起動・停止の遠隔操作（daemon）
