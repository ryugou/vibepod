# VibePod Phase 0: Slack Bridge — Design Spec

## Overview

VibePod の `run` コマンドに `--bridge` フラグを追加し、コンテナ出力の無音検知 → Slack 通知 → Slack/ターミナルからの応答 → コンテナ stdin 送信 を実現する。ターミナルでの透過表示を維持しつつ、離席時は Slack がバックアップとなるハイブリッド型。

`--bridge` は interactive モード（引数なし起動）でも fire-and-forget モード（`--prompt`）でも使用可能。どちらでも Claude Code が確認待ちになる可能性があり、またタスク完了通知としても有用。

Phase 0 の目的は、入力待ちパターンの知見蓄積と離席時の簡易応答の実用化。

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
│     ├── resize          : SIGWINCH → resize_container_tty API
│     └── signal          : Ctrl+C / コンテナ終了検知
│
└── --bridge なしの場合
      └── 既存の docker run -it / bollard stream_logs パス（変更なし）
```

### Design Decisions

- **VibePod サブコマンドとして統合** — 別バイナリや別クレートにせず、`vibepod run --bridge` として実装。コード共有が容易。
- **bollard attach_container** — コンテナ側に pty を割り当て（tty=true）、bollard の attach API で stdin/stdout ストリームを取得。`portable-pty` + `docker run -it` の二重 pty 問題を回避。既存の bollard 依存を活用。
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
- `SIGWINCH` シグナルを監視し、bollard の `resize_container_tty` API でコンテナ側 pty のウィンドウサイズを同期

### Existing Code Integration

- `src/cli/run.rs` の `execute()` 内で `--bridge` 判定
- bridge あり → `bridge::run()` に委譲（Docker 起動引数の構築は既存コードと共有）
  - interactive モードでも bollard attach を使用（既存の `docker` CLI サブプロセスパスとは異なる実行パス）
- bridge なし → 既存の実行パス（変更なし）

## Idle Detection & Notification Flow

### State Machine

```
コンテナ出力あり → バッファ蓄積 + タイマーリセット
                       │
                 N秒間出力なし
                       │
                       ▼
                 バッファ内容を取得
                 （最大40行、超過は末尾切り詰め）
                       │
                 ターミナルから入力あり？
                 ├── はい → バッファクリア、通知キャンセル
                 └── いいえ → Slack に通知送信
                               │
                               ▼
                       応答待ち状態
                       ├── ターミナル stdin → コンテナに転送、通知を「応答済み」に更新
                       ├── Slack ボタン/リアクション/スレッド返信 → コンテナに転送、同上
                       │
                       先着1つを採用（AtomicBool ガードで即ロック、以降の書き込みは drop）
                       │
                       ▼
                       バッファクリア、次の無音検知へ
```

### States

- **Buffering** — 出力を蓄積中。出力がある度にタイマーリセット
- **Idle** — N秒無音。Slack 通知を送信
- **WaitingResponse** — 通知済み、応答待ち。ターミナル or Slack どちらからでも応答可能

### No Distinction Between Input-Waiting and Task Completion

入力待ちとタスク完了を区別しない。どちらも「出力が止まった。最後の状態はこれです」という統一通知。ユーザーが文脈で判断し、必要なら応答、不要なら無視する。

### Output Buffer

- 無音期間で区切る（前回の無音から今回の無音までが1ブロック）
- 最大40行（Slack Block Kit の 3,000 文字制限に収まる範囲）
- 超過時は末尾40行に切り詰め、先頭に `... (N lines truncated)` を表示
- Slack 送信時は ANSI エスケープシーケンスをストリップする（`strip-ansi-escapes` クレート使用）

## Slack Communication

### Connection

Socket Mode（WebSocket）で接続。HTTP エンドポイント不要。

接続断時は exponential backoff で再接続を試みる。再接続不可の場合は stderr に警告を出し、ターミナル専用モードにフォールバック（bridge なしと同等の動作を継続）。

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

- Start: `🟢 VibePod セッション開始 (project-name)`
- End: `🔴 VibePod セッション終了 (project-name, exit code: N)`

exit code はコンテナの終了コード（bollard `wait_container` API で取得）。

### Slack Crate Selection

`slack-morphism` のメンテナンス状況を実装時に確認。判断基準：
- crates.io での最終リリースが6ヶ月以内か
- 現行の Slack API バージョンに対応しているか

メンテされていれば採用、されていなければ `reqwest` + `tokio-tungstenite` で Slack Web API / Socket Mode を薄くラップする自前実装。

## Logging

知見蓄積のため、最小限のイベントを JSON Lines で記録。

### Output Path

`{config_dir}/vibepod/bridge-logs/{session-id}.jsonl`

（`config_dir` は `dirs::config_dir()` で決定。macOS: `~/Library/Application Support/vibepod/bridge-logs/`）

### Events

```jsonl
{"ts":"2026-03-25T14:35:30+09:00","event":"notified","last_lines":"Do you want to proceed? (y/n)"}
{"ts":"2026-03-25T14:36:05+09:00","event":"responded","source":"slack_button","stdin_sent":"y\n","response_time_seconds":35}
```

- `notified` — Slack 通知送信時。`last_lines` に表示した出力を記録
- `responded` — 応答受信時。`source`（terminal / slack_button / slack_reaction / slack_thread）、送信内容、応答時間を記録

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
vibepod run --bridge --env-file ./custom.env
```

### Configuration

Bridge 用の環境変数はデフォルトで `{config_dir}/vibepod/bridge.env` から読み込む。`op://` 参照にも対応（既存の 1Password CLI 連携を流用）。

```bash
# {config_dir}/vibepod/bridge.env
SLACK_BOT_TOKEN="op://ai-agents/slack-bridge/bot-token"
SLACK_APP_TOKEN="op://ai-agents/slack-bridge/app-token"
SLACK_CHANNEL_ID=C0AJHQRE23Z
```

優先順位:
1. `--slack-channel` CLI オプション（最優先）
2. `--env-file` で指定したファイル内の `SLACK_CHANNEL_ID`
3. `{config_dir}/vibepod/bridge.env` 内の `SLACK_CHANNEL_ID`

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
│   ├── detector.rs   # 無音検知、バッファ管理、状態遷移、ANSI ストリップ
│   ├── slack.rs      # Socket Mode 接続、通知送信、応答受信、再接続
│   └── logger.rs     # JSON Lines ログ記録
├── cli/
│   ├── mod.rs        # --bridge, --notify-delay, --slack-channel 追加
│   └── run.rs        # --bridge 判定で bridge::run() に分岐
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
```

Slack クレートは実装時に選定（slack-morphism or reqwest + tokio-tungstenite）。

## Slack App Setup Requirements

- Bot Token Scopes: `chat:write`, `reactions:read`, `channels:history`
- Socket Mode: 有効化
- Interactivity: ON
- Event Subscriptions: `reaction_added`, `message`

## Out of Scope (v2+)

- 後付け bridge attach（daemon 化で対応）
- pty 出力の全文ストリーミング（Dashboard）
- 複数セッションの並行管理（daemon）
- APNs プッシュ通知（Mobile App）
- セッションの起動・停止の遠隔操作（daemon）
