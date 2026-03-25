# VibePod Phase 0: Slack Bridge

## 概要

Claude Code の pty 出力を監視し、入力待ちを検知したら Slack に通知。Slack からの応答（ボタン / リアクション / テキスト）を Claude Code の stdin に送信するブリッジツール。

VibePod v2 の daemon 化に先立ち、入力待ちパターンの知見蓄積と離席時の簡易応答を実現する。

## アーキテクチャ

```
Claude Code (pty) ←→ Slack Bridge Process ←→ Slack API (Socket Mode)
                                                    ↕
                                              iPhone / Apple Watch
                                         （Slack アプリ経由で操作）
```

## 動作フロー

### 1. 起動

- Bridge Process が Claude Code をpty 経由で起動（または既存の pty セッションにアタッチ）
- Slack Socket Mode で WebSocket 接続を確立
- 指定された Slack チャンネル（またはDM）に「セッション開始」を通知

### 2. 通常時（出力の転送）

- pty 出力をリアルタイムで監視
- 全出力をそのまま転送するとノイズが多すぎるため、以下のいずれかの方式を採用：
  - **方式A**: 一定間隔（例: 30秒）でバッファをまとめて投稿（サマリー的）
  - **方式B**: 出力は転送せず、入力待ち検知時のみ通知
- 初期実装は**方式B**を推奨（シンプル＆通知ノイズが少ない）

### 3. 入力待ち検知

以下の条件の AND で判定する：

- pty 出力が一定時間（例: 3秒）停止している
- 最終行が改行で終わっていない（プロンプトが表示されている状態）

さらに、以下のパターンマッチで検知精度を上げる：

```
# 高信頼パターン（ほぼ確実に入力待ち）
(y/n)
(Y/n)
(yes/no)
? (Use arrow keys)
? (y/N)
Enter a value:
Press Enter to continue

# 中信頼パターン（文脈次第）
末尾が ": " で終わる
末尾が "? " で終わる
末尾が "> " で終わる
```

### 4. Slack への通知

入力待ちを検知したら、以下の形式で Slack に投稿する：

```json
{
  "channel": "<設定されたチャンネルID>",
  "text": "🤖 Claude Code が入力待ちです",
  "blocks": [
    {
      "type": "section",
      "text": {
        "type": "mrkdwn",
        "text": "🤖 *Claude Code が入力待ちです*\n```\n<直近5行の出力>\n```"
      }
    },
    {
      "type": "actions",
      "block_id": "input_response",
      "elements": [
        {
          "type": "button",
          "text": { "type": "plain_text", "text": "Yes" },
          "action_id": "respond_yes",
          "style": "primary"
        },
        {
          "type": "button",
          "text": { "type": "plain_text", "text": "No" },
          "action_id": "respond_no",
          "style": "danger"
        },
        {
          "type": "button",
          "text": { "type": "plain_text", "text": "Skip" },
          "action_id": "respond_skip"
        }
      ]
    }
  ]
}
```

### 5. Slack からの応答受信

以下の3つの入力手段をすべてサポートする：

#### ボタンクリック（Block Kit Interactive）

- `respond_yes` → stdin に `y\n` を送信
- `respond_no` → stdin に `n\n` を送信
- `respond_skip` → stdin に `s\n` を送信（スキップ操作がある場合）
- ボタン押下後、元メッセージを更新して「✅ Yes と応答しました」等に書き換える

#### リアクション（Apple Watch 向け）

- 👍 → `y\n`
- 👎 → `n\n`
- ⏭️ → `s\n`
- リアクション検知後、スレッドに「✅ リアクションで Yes と応答しました」と投稿

#### テキスト返信（自由入力）

- 通知メッセージのスレッドに返信されたテキストをそのまま stdin に送信
- 末尾に改行がなければ付加する

**重複防止**: ボタン・リアクション・テキストのいずれかで最初に応答されたものを採用し、それ以降の入力は無視する（同一の入力待ちに対して二重送信しない）。

### 6. 応答後

- stdin に入力を送信
- 次の入力待ちまで監視を継続
- セッション終了（Claude Code のプロセス終了）を検知したら Slack に完了通知

## 技術スタック

### 言語

Rust を推奨（VibePod 本体と統合しやすい）。代替として Deno/TypeScript も可。

### 主要ライブラリ（Rust の場合）

- `portable-pty` or `pty-process`: pty 制御
- `slack-morphism`: Slack API + Socket Mode
- `tokio`: 非同期ランタイム
- `serde` / `serde_json`: Slack メッセージの構築・パース

### Slack 設定要件

- Slack App を作成（Bot Token Scopes が必要）
  - `chat:write` — メッセージ投稿
  - `reactions:read` — リアクション検知
  - `channels:history` — スレッド返信の読み取り
- Socket Mode を有効化（Incoming Webhook 不要、WebSocket でリアルタイム双方向通信）
- Interactivity をON（ボタンクリックの受信に必要）
- Event Subscriptions: `reaction_added`, `message`（スレッド返信検知用）

### 環境変数

```
SLACK_BOT_TOKEN=xoxb-...
SLACK_APP_TOKEN=xapp-...     # Socket Mode 用
SLACK_CHANNEL_ID=C0AJHQRE23Z # 投稿先チャンネル（#ai_agents or 専用チャンネル）
```

## CLI インターフェース

```bash
# Claude Code をラップして起動
slack-bridge run -- claude-code --dangerously-skip-permissions

# 既存の vibepod セッションと組み合わせる場合
slack-bridge run -- vibepod run <session>

# 設定確認
slack-bridge config
```

## 投稿先チャンネルの方針

- 既存の `#ai_agents` (C0AJHQRE23Z) に投稿するか、専用チャンネル（例: `#claude-bridge`）を新設するかは運用判断
- 専用チャンネルのほうがノイズが少なく、スレッド管理もしやすい

## 知見蓄積のためのログ

Phase 0 の目的の一つは v2 の設計へのフィードバック。以下をログとして記録し、後で分析できるようにする：

- 入力待ちの種類（検知に使ったパターン）
- 応答手段（ボタン / リアクション / テキスト）
- 応答までの所要時間
- 誤検知の有無

ログは JSON Lines 形式でローカルファイルに出力する。

## スコープ外（v2 以降）

- pty 出力の全文ストリーミング → v2 Dashboard
- 複数セッションの並行管理 → v2 daemon
- APNs プッシュ通知 → v2 Mobile App
- セッションの起動・停止の遠隔操作 → v2 daemon
