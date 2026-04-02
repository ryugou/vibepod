# VibePod v2: Dashboard & Client App Vision

## 背景・課題

VibePod v1 はターミナル内で完結する CLI ツール。
Claude Code が入力待ち（y/n 確認、ファイルパス入力等）になった場合、そのターミナルセッションにアクセスできる環境（Mosh + tmux 等）がなければ応答できない。

特に以下のケースで課題がある：

- 離席中に Claude Code が入力待ちになり、タスクがブロックされる
- スマホ（iPhone）や Apple Watch から簡易応答したい
- 複数セッションの状態を一覧で把握したい

## 設計方針: Daemon アーキテクチャ

v2 では `vibepod run` を daemon 化し、すべてのクライアントが同一 API を通じて接続する構造にする。

```
vibepod v2
  └── vibepod daemon（常駐プロセス）
        ├── コンテナライフサイクル管理
        ├── pty 入出力の中継
        ├── 入力待ち検知
        └── API（REST + WebSocket）
              ↑
        ┌─────┼──────────────┐
        │     │              │
       CLI  Dashboard      Mobile App
            (Web UI)     (iPhone / Apple Watch)
```

### 核心: API をパブリック API として設計する

Dashboard 専用の内部 API として作らない。最初から「VibePod 自体のパブリック API」として設計し、すべてのクライアント（CLI / Web / Mobile）が同じエンドポイントを使う。これにより：

- クライアント追加時に二重実装が不要
- サードパーティ連携（Slack 等）も同じ API 上に構築可能
- CLI（`vibepod run`）も daemon に接続するクライアントの一つになり、実装がシンプルになる

## Dashboard（Web UI）

v2 の中核機能。iPhone の Safari（Tailscale 経由）から、vibepod で起動した Claude Code の状態確認と操作を行う。

### 動機

Slack 通知 + StreamDeck の組み合わせでは、許可ダイアログの詳細が取れない・選択肢が限定されるなどの制限があり、実用に耐えない。Mosh クライアントならフル操作可能だが、Claude Code 操作に特化した UI ではない。ターミナルの丸ごと表示ではなく、会話と許可ダイアログを整形して表示し、ボタンで応答できる UI が必要。

### 技術方針

- vibepod がホストする Web サーバ（Rust）に Web UI を組み込む
- Claude Code のトランスクリプト（JSONL）をリアルタイムで読み、会話内容を表示する
- ボタン押下時は `tmux send-keys` でコンテナ内の Claude Code に入力を送る
- Tailscale 経由でアクセスするため、認証は Tailscale のネットワーク境界に依存する（追加認証は後回し）

### トランスクリプトから取れる情報（検証済み）

- assistant メッセージのテキスト（Claude Code の発言）
- tool_use の name と input（Write のファイルパス、Bash のコマンド内容など）
- **取れないもの**: 許可ダイアログの選択肢テキスト（UI 側で生成されるため JSONL に含まれない）

### 操作方法（検証済み）

- `tmux send-keys -t :<window> Enter` → Yes（デフォルト選択を送信）
- `tmux send-keys -t :<window> S-Tab` → Always（未検証、要確認）
- `tmux send-keys -t :<window> Escape` → No（未検証、要確認）
- フルパス指定が必要（例: `/opt/homebrew/bin/tmux`）

### 段階的な進め方

1. **Step 1**: トランスクリプトの末尾を表示 + Yes/No ボタンだけの最小 UI
2. **Step 2**: リアルタイム更新（WebSocket or SSE）
3. **Step 3**: 複数セッションの一覧、ログ検索、通知設定の UI 化など → Dashboard へ育てる

### 検証で得た知見

- Claude Code の hooks は `Notification` イベントで `notification_type` と `message` を渡してくる
- `notification_type: "permission_prompt"` の時、`transcript_path` から JSONL を読むと詳細が取れる
- ただし hooks 経由だと情報が限定的で、トランスクリプト直読みの方が確実
- StreamDeck Mobile（iOS）からシェルスクリプト実行は不可（引数も渡せない）
- Mosh 接続 + tmux attach で iPhone からフル操作は可能（La Terminal で検証済み）

## Mobile App（iPhone / Apple Watch）

Dashboard のクライアントの一つという位置づけ。専用設計ではなく、同じ WebSocket / REST API に接続する。

### iPhone App

- セッション一覧・状態確認
- pty 出力の閲覧
- 入力待ちへの応答（テキスト入力 + ボタン選択）
- APNs プッシュ通知（入力待ち検知時に即時通知）

### Apple Watch App

- 入力待ち通知（Haptic で手首に通知、Slack より低レイテンシ）
- 選択式応答（Yes / No / Skip 等のボタン UI）
- Digital Crown でのスクロール選択（選択肢が多い場合）
- 音声入力による短いテキスト応答
- コンプリケーション: セッション稼働状態を文字盤に表示

### 技術スタック（想定）

- Swift UI（Watch / iPhone 共通）
- WatchConnectivity（Watch ↔ iPhone 連携）
- APNs 通知: daemon から直接、または Firebase Cloud Messaging 経由
- 通信: Tailscale 上の daemon WebSocket エンドポイントに接続

## 入力待ち検知ロジック

daemon 側で pty 出力を監視し、以下のヒューリスティクスで入力待ちを検知する：

- 一定時間（数秒）出力が停止している
- 最終行が改行で終わっていない（プロンプト表示中）
- 既知のパターンマッチ（`? `, `(y/n)`, `(Y/n)` 等）

検知後、最終行（質問文）を抽出し、選択肢があればパースしてクライアントに構造化データとして配信する。

## 将来展望: エージェント司令塔

VibePod が複数エージェントの管理基盤に成長した場合、同じ daemon + API アーキテクチャで以下に拡張できる：

- 複数エージェント（シャア、シビュラ、MAGI、自来也等）の稼働状態を統合監視
- 異常検知時の通知と簡易介入
- エージェント起動・停止の遠隔操作

Apple Watch は「エージェント司令塔ウォッチ」として、手首から全エージェントの状態把握と簡易指示が可能になる。

## 実装の段階的アプローチ

1. **Phase 0（現状の Slack ベース簡易対応）**: Slack リアクション方式で離席時の簡易応答を実用化。入力待ちパターンの知見を蓄積する
2. **Phase 1（daemon 化 + API）**: `vibepod run` を daemon + CLI クライアント構成に分離。REST + WebSocket API を定義
3. **Phase 2（Dashboard）**: Web UI を構築。セッション管理・pty ストリーミング・応答 UI
4. **Phase 3（Mobile App）**: iPhone / Apple Watch アプリ。APNs 通知統合

Phase 0 で得た知見（どんな入力待ちが多いか、どういう応答 UI が実用的か）を Phase 1 以降の設計にフィードバックする。

## 将来課題: 後付け Bridge Attach

Phase 0 では `vibepod run --bridge` を起動時に指定する必要がある。しかし実運用では「bridge なしで起動したが、後から Slack 通知が欲しくなった」ケースが発生する（急な外出、想定より長いタスク等）。

Phase 1（daemon 化）で以下を実現する：

- 実行中セッションへの bridge 後付け接続（`vibepod bridge attach`）
- daemon が常に pty を管理するため、クライアント（CLI / bridge / Dashboard）の接続・切断が自由になる
- Phase 0 で pty 自前管理の知見を蓄積し、daemon 化の設計にフィードバックする
