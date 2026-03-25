# VibePod Phase 0: Slack Bridge — Design Spec

## Overview

VibePod の `run` コマンドに `--bridge` フラグを追加し、pty 出力の無音検知 → Slack 通知 → Slack/ターミナルからの応答 → pty stdin 送信 を実現する。ターミナルでの透過表示を維持しつつ、離席時は Slack がバックアップとなるハイブリッド型。

Phase 0 の目的は、入力待ちパターンの知見蓄積と離席時の簡易応答の実用化。

## Architecture

```
vibepod run --bridge --env-file .env.slack
│
├── DockerRuntime (既存)
│     └── コンテナ起動（tty=true, open_stdin=true）
│
├── PtyBridge (新規: src/bridge/)
│     ├── portable-pty で Master/Slave ペア作成
│     ├── Docker プロセスの stdin/stdout を Slave 側に接続
│     └── Master 側を VibePod が制御
│
├── tokio runtime
│     ├── pty_read  : Master → stdout 転送（透過）+ バッファ蓄積
│     ├── pty_write : ターミナル stdin → Master 転送
│     ├── idle_watch: 無音 N 秒検知 → Slack 通知トリガー
│     ├── slack_out : Slack Socket Mode → 通知送信
│     ├── slack_in  : Slack 応答受信 → Master に stdin 書き込み
│     └── signal    : Ctrl+C / プロセス終了検知
│
└── --bridge なしの場合
      └── 既存の docker run -it パス（変更なし）
```

### Design Decisions

- **VibePod サブコマンドとして統合** — 別バイナリや別クレートにせず、`vibepod run --bridge` として実装。コード共有が容易。
- **シングルプロセス・マルチタスク** — tokio タスクで pty 読み書き、無音検知、Slack 通信を並行処理。IPC 不要でシンプル。
- **後付け bridge attach は Phase 0 スコープ外** — daemon 化（v2）で対応予定。docs/proposal-v2-dashboard.md に将来課題として記録済み。

## PTY Management

```
┌─ VibePod プロセス ───────────────────┐
│                                      │
│  portable-pty::PtyPair               │
│    master ←→ slave                   │
│      │          │                    │
│      │          └── docker run -it   │
│      │              の stdin/stdout   │
│      │              に接続            │
│      │                               │
│      ├── read → ターミナル stdout     │
│      ├── read → バッファ蓄積          │
│      └── write ← ターミナル stdin     │
│               ← Slack 応答           │
└──────────────────────────────────────┘
```

- `portable-pty` で pty ペアを作成し、`CommandBuilder` で `docker run -it ...` を子プロセスとして起動
- master の read 側を `tokio::io::AsyncRead` でラップして非同期読み取り
- 読み取ったバイト列はそのまま stdout に書き出し（ANSI 完全透過）、同時にバッファにもコピー
- ターミナルの stdin は raw mode に設定し、キー入力をそのまま master の write 側に転送
- Slack からの応答も同じ master write に書き込む（channel 経由）

### Existing Code Integration

- `src/cli/run.rs` の `execute()` 内で `--bridge` 判定
- bridge あり → `bridge::run()` に委譲（Docker 起動引数の構築は共有）
- bridge なし → 既存の `Command::new("docker").exec()` パス（変更なし）

## Idle Detection & Notification Flow

### State Machine

```
pty 出力あり → バッファ蓄積 + タイマーリセット
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
                    ├── ターミナル stdin → pty に転送、通知を「応答済み」に更新
                    ├── Slack ボタン/リアクション/スレッド返信 → pty に転送、同上
                    │
                    先着1つを採用、以降は無視
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

## Slack Communication

### Connection

Socket Mode（WebSocket）で接続。HTTP エンドポイント不要。

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

### Slack Crate Selection

`slack-morphism` のメンテナンス状況を実装時に確認。メンテされていれば採用、されていなければ `reqwest` + `tokio-tungstenite` で Slack Web API / Socket Mode を薄くラップする自前実装。

## Logging

知見蓄積のため、最小限のイベントを JSON Lines で記録。

### Output Path

`~/.vibepod/bridge-logs/{session-id}.jsonl`

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

### Usage Example

```bash
vibepod run --bridge --notify-delay 30
vibepod run --bridge --slack-channel C0AJHQRE23Z
vibepod run --bridge --env-file ./custom.env --notify-delay 10
```

### Configuration

Bridge 用の環境変数はデフォルトで `~/.config/vibepod/bridge.env` から読み込む。`op://` 参照にも対応（既存の 1Password CLI 連携を流用）。

```bash
# ~/.config/vibepod/bridge.env
SLACK_BOT_TOKEN="op://ai-agents/slack-bridge/bot-token"
SLACK_APP_TOKEN="op://ai-agents/slack-bridge/app-token"
SLACK_CHANNEL_ID=C0AJHQRE23Z
```

優先順位:
1. `--slack-channel` CLI オプション（最優先）
2. `--env-file` で指定したファイル内の `SLACK_CHANNEL_ID`
3. `~/.config/vibepod/bridge.env` 内の `SLACK_CHANNEL_ID`

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
│   ├── pty.rs        # pty ペア作成、Docker プロセス起動、read/write 管理
│   ├── detector.rs   # 無音検知、バッファ管理、状態遷移
│   ├── slack.rs      # Socket Mode 接続、通知送信、応答受信
│   └── logger.rs     # JSON Lines ログ記録
├── cli/
│   ├── mod.rs        # --bridge, --notify-delay, --slack-channel 追加
│   └── run.rs        # --bridge 判定で bridge::run() に分岐
└── ...               # 既存モジュール（変更なし）
```

モジュール間は tokio の channel (`mpsc`) で疎結合に接続：
```
pty.rs → (出力バイト列) → detector.rs → (通知トリガー) → slack.rs
slack.rs → (応答テキスト) → pty.rs
```

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
