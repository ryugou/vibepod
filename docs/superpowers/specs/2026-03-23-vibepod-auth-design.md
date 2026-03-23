# vibepod login / コンテナ認証モデル 設計仕様

## 概要

Docker コンテナ内の Claude Code が OAuth 認証を独立して行えるようにする。ホスト側の OAuth セッションとの競合を防ぎ、複数コンテナの同時実行も可能にする。

## 背景

- Claude Code の OAuth トークンはリフレッシュ時に更新される
- ホストとコンテナが同じ credentials を共有すると、一方のリフレッシュがもう一方を無効化する
- `~/.claude/` 全体をマウントすると、プラグインキャッシュ等の読み込みで Claude Code がハングする

## 認証モデル

### セッションの種類

| 種類 | 保存先 | 用途 |
|------|--------|------|
| 共有セッション | `~/.config/vibepod/auth/credentials.json` | `vibepod login` で取得。全プロジェクト共通。1つのみ |
| コンテナ専用セッション | `<project>/.vibepod/auth/containers/<name>.json` | `--isolated` 指定時。プロジェクトローカル |

### フロー

#### vibepod login

1. `--network host` でコンテナを起動
2. コンテナ内で `claude /login` を実行
3. Claude Code が URL を出力 → vibepod がキャプチャしてホスト側で `open <URL>` を実行
4. ユーザーがブラウザで認可
5. コールバックが `--network host` 経由でコンテナ内のサーバーに届く
6. コンテナ内の `~/.claude/.credentials.json` を `~/.config/vibepod/auth/credentials.json` にコピー（パーミッション 600）
7. コンテナ破棄

#### vibepod run（通常）

1. `~/.config/vibepod/auth/credentials.json` が存在するか確認
2. 存在しない → 「`vibepod login` を先に実行してください」エラー
3. トークンの有効期限を `expiresAt` フィールドで確認。失効していれば「`vibepod login` を再実行してください」エラー
4. 共有セッションが使用中か確認（ロックファイル）
5. 未使用 → 共有セッションをコピーしてコンテナに渡す。ロックを取得
6. 使用中 → 「共有セッションは使用中です。`vibepod run --isolated` を使用してください」とメッセージを出して中止
7. コンテナ終了時に一時ファイルから元ファイルに書き戻し → その後ロックを解放（この順序を厳守）

#### vibepod run --isolated

1. `--isolated` 時のコンテナ名は `vibepod-<project>-isolated` 固定。`--name <name>` で別名を指定可能
2. `.vibepod/auth/containers/<name>.json` が存在するか確認
3. 存在する → トークンの有効期限を確認。有効ならコピーしてコンテナに渡す。失効していれば再ログインを確認
4. 存在しない → `--network host` でコンテナを起動し `claude /login` を実行（URL キャプチャ方式）→ `.vibepod/auth/containers/` に永続化（パーミッション 600）→ コンテナ破棄後、通常のネットワーク設定でコンテナを再起動
5. 共有セッションのロックは取らない
6. `--no-network` が指定されている場合、ログインフロー中は一時的にネットワークを有効化し、ログイン完了後に `--no-network` でコンテナを再起動

#### vibepod logout

1. `~/.config/vibepod/auth/credentials.json` を削除
2. ロックファイルがあれば強制解放
3. `--all` オプションで `.vibepod/auth/containers/` 内の全コンテナ専用セッションも削除

### credentials のコンテナへの渡し方

- 元ファイル（credentials.json または containers/*.json）を直接マウントしない
- 一時ファイルにコピーし、コンテナに `/home/vibepod/.claude/.credentials.json` として read-write でマウント
- コンテナ内でトークンリフレッシュが走っても元ファイルに影響しない
- コンテナ終了時に一時ファイルから元ファイルに書き戻し → その後ロックを解放（この順序を厳守）
- 一時ファイルはコンテナ終了後に削除

### ロック機構

- `~/.config/vibepod/auth/credentials.lock` にロックファイルを作成
- ロックファイルにはコンテナ名を記載
- コンテナ終了時（正常・異常問わず）に書き戻し完了後にロックを解放
- ロックファイルが残っている場合、記載されたコンテナ名で `docker inspect` を実行し、コンテナの生存を確認。コンテナが存在しなければ stale ロックとして自動削除

## URL キャプチャとブラウザ起動

`vibepod login` および `--isolated` の初回ログイン時のみ使用。`vibepod run` の通常フローでは使用しない。

コンテナ内の `claude /login` の出力を vibepod がパイプで監視し：

1. URL パターン（`https://platform.claude.com/oauth/...` 等）を検出
2. ホスト側で `open <URL>`（macOS）/ `xdg-open <URL>`（Linux）を実行してブラウザを起動
3. ブラウザが開けない場合は URL をそのまま表示してユーザーに手動対応を促す
4. その他の出力はそのままターミナルに中継

## CLI インターフェース

### vibepod login

```
vibepod login
```

- 引数なし
- 共有セッションが既にある場合「既存のセッションを上書きしますか？」と確認
- ログイン成功後、セッション情報を表示

### vibepod logout

```
vibepod logout
vibepod logout --all
```

- 引数なし: 共有セッションとロックを削除
- `--all`: 共有セッション + 全コンテナ専用セッションを削除

### vibepod run --isolated

```
vibepod run --isolated
vibepod run --isolated --name my-session
```

- 既存オプションに `--isolated` を追加
- コンテナ専用セッションを使用（または新規作成）
- `--name` でセッション名を指定可能（デフォルト: `vibepod-<project>-isolated`）

### CLI 出力イメージ（vibepod login）

```
  ┌  VibePod Login
  │
  ◇  コンテナ用の認証セッションを作成します
  │
  ◇  ブラウザが開きます。ログインしてください...
  │
  ◆  認証を待っています... (Ctrl+C でキャンセル)
  │
  ◇  認証完了！ ryugo@sivira.co
  │  セッションを保存しました: ~/.config/vibepod/auth/credentials.json
  └
```

### CLI 出力イメージ（共有セッション使用中）

```
  ┌  VibePod
  │
  ◇  Detected git repository: my-project
  │
  ⚠  共有セッションは別のコンテナ (vibepod-other-abc123) で使用中です。
  │  `vibepod run --isolated` を使用してください。
  └
```

## エラーハンドリング

| 状況 | 挙動 |
|------|------|
| `vibepod login` 未実行で `vibepod run` | 「`vibepod login` を先に実行してください」 |
| 共有セッションのトークンが失効（`expiresAt` で判定） | 「セッションの有効期限が切れています。`vibepod login` を再実行してください」 |
| `--isolated` でトークン失効 | 「セッションの有効期限が切れています。再ログインしますか？」と確認 |
| 共有セッションが使用中 | 「`vibepod run --isolated` を使用してください」と案内して中止 |
| URL キャプチャに失敗 | URL をそのまま表示してユーザーに手動対応を促す |
| ロックファイルが stale（コンテナが存在しない） | 自動でロック解放して続行 |
| `claude /login` がコンテナ内で失敗 | エラーメッセージをそのまま表示 |
| ブラウザが開けない（SSH 等） | URL を表示して手動でコピペを促す |
| `--no-network` + `--isolated` で初回ログイン | ログインフロー中のみネットワーク有効化、完了後に `--no-network` で再起動 |

## 変更対象ファイル

### 新規作成
- `src/cli/login.rs` — login コマンドの実装
- `src/cli/logout.rs` — logout コマンドの実装
- `src/auth.rs` — 認証セッション管理（保存・読み込み・ロック・コピー・有効期限チェック）

### 変更
- `src/cli/mod.rs` — `Login`, `Logout` サブコマンドと `--isolated`, `--name` オプション追加
- `src/main.rs` — login, logout コマンドのルーティング
- `src/cli/run.rs` — 認証フロー組み込み（共有セッション確認、ロック、コピー、書き戻し）。既存の `~/.claude/.credentials.json` チェックを `vibepod auth` に置き換え
- `src/lib.rs` — `auth` モジュール追加

## テスト方針

### ユニットテスト
- 認証セッションの保存・読み込み（パーミッション 600 確認含む）
- ロックファイルの作成・解放・stale 検出（docker inspect ベース）
- URL パターン検出
- トークン有効期限チェック（`expiresAt` パース）

### 統合テスト
- `vibepod login` / `vibepod logout` / `vibepod run --isolated` の CLI パーステスト
- エラーハンドリングの各パターン

### 手動テスト（TTY 操作が必要）
- `vibepod login` → ブラウザ認可 → セッション保存の E2E フロー
- `vibepod run` → 認証成功で Claude Code が使える
- 2つ目の `vibepod run` → `--isolated` への案内表示
- `vibepod run --isolated` → コンテナ専用ログイン → セッション永続化
- `vibepod logout` → セッション削除確認
