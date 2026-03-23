# vibepod login / コンテナ認証モデル 設計仕様

## 概要

Docker コンテナ内の Claude Code が独立した長期 OAuth トークンで認証できるようにする。ホスト側の OAuth セッションとの競合を防ぎ、複数コンテナの同時実行も可能にする。

## 背景

- Claude Code の OAuth トークンはリフレッシュ時に更新される
- ホストとコンテナが同じ credentials を共有すると、一方のリフレッシュがもう一方を無効化する
- `~/.claude/` 全体をマウントすると、プラグインキャッシュ等の読み込みで Claude Code がハングする
- `claude auth login` はコンテナ内の TTY でインタラクティブ入力が動作しない

## 認証方式

`claude setup-token` コマンドで 1 年有効の長期 OAuth トークンを生成し、環境変数 `CLAUDE_CODE_OAUTH_TOKEN` でコンテナに渡す。

- credentials ファイルのマウント不要
- ロック機構不要（環境変数なのでファイル競合が発生しない）
- 複数コンテナで同じトークンを同時使用可能
- トークンはホストの OAuth セッションとは独立

### トークンの保存先

`~/.config/vibepod/auth/token.json`（パーミッション 600）

```json
{
  "token": "sk-ant-oat01-...",
  "created_at": "2026-03-23T12:00:00+00:00",
  "expires_at": "2027-03-23T12:00:00+00:00"
}
```

## フロー

### vibepod login

1. コンテナをバックグラウンドで起動（`docker run -d --network host`）
2. コンテナ内に偽の `/usr/bin/xdg-open` スクリプトをマウント（URL をファイルに書き出す）
3. バックグラウンドスレッドが 0.5 秒間隔でコンテナ内の URL ファイルを監視
4. `docker exec -it` で `bash -c "script -q <file> -c 'claude setup-token'"` を実行
5. `claude setup-token` が `xdg-open` を呼ぶ → URL がファイルに書かれる → ホスト側で `open` コマンドでブラウザ起動
6. ユーザーがブラウザで認可
7. コールバックが `--network host` 経由でコンテナ内のサーバーに到達
8. トークンが生成・表示される
9. `script` の出力ファイルからトークン（`sk-ant-` パターン）を正規表現で抽出
10. `~/.config/vibepod/auth/token.json` に保存（パーミッション 600）
11. コンテナ削除

### vibepod run

1. `~/.config/vibepod/auth/token.json` が存在するか確認
2. 存在しない → 「`vibepod login` を先に実行してください」エラー
3. トークンの有効期限を確認。残り 7 日以内 → 「`vibepod login` を再実行してください」エラー（強制更新）
4. 環境変数 `CLAUDE_CODE_OAUTH_TOKEN` にトークンをセットしてコンテナに渡す

### vibepod logout

1. `~/.config/vibepod/auth/token.json` を削除

## ブラウザ自動起動

`vibepod login` 時にコンテナ内からホストのブラウザを自動起動する仕組み。

コンテナ内にはブラウザが存在しないため、偽の `xdg-open` スクリプトをマウントして URL をキャプチャし、ホスト側で `open`（macOS）/ `xdg-open`（Linux）を実行する。

1. 偽スクリプト: `#!/bin/sh\necho "$1" > /tmp/vibepod-oauth-url`
2. `/usr/bin/xdg-open` としてコンテナにマウント
3. バックグラウンドスレッドが `docker exec cat /tmp/vibepod-oauth-url` で URL ファイルを 0.5 秒間隔でポーリング
4. URL を検出したらホスト側で `open` コマンドを実行

`setup-token` の TUI 出力にはカーソル移動が含まれるため、stdout パースによる URL 検出は困難。偽 `xdg-open` 方式はこの問題を回避する。

## トークンの有効期限管理

- `setup-token` で生成されるトークンは 1 年有効
- `vibepod run` 起動時に残り 7 日以内の場合、`vibepod login` の再実行を強制する
- これにより、作業中にトークンが切れるリスクを実質的に排除（1 週間連続稼働しない限り）

## CLI インターフェース

### vibepod login

```
vibepod login
```

- 引数なし
- 既存のトークンがある場合「上書きしますか？」と確認
- ブラウザが自動で開く。開かない場合は URL を手動コピペ

### vibepod logout

```
vibepod logout
```

- トークンを削除

### CLI 出力イメージ（vibepod login）

```
  ┌  VibePod Login
  │
  ◇  コンテナ用の長期トークンを作成します
  │
  (claude setup-token の TUI が表示される)
  │
  ◇  認証完了！
  └
```

## エラーハンドリング

| 状況 | 挙動 |
|------|------|
| `vibepod login` 未実行で `vibepod run` | 「`vibepod login` を先に実行してください」 |
| トークンの有効期限が残り 7 日以内 | 「`vibepod login` を再実行してください」（強制） |
| `claude setup-token` がコンテナ内で失敗 | エラーメッセージをそのまま表示 |
| トークンが出力から抽出できない | 「トークンが出力から見つかりませんでした」 |
| ブラウザが開けない（SSH 等） | URL が TUI に表示されるので手動コピペ |

## 変更対象ファイル

### 新規作成
- `src/cli/login.rs` — login コマンドの実装
- `src/cli/logout.rs` — logout コマンドの実装
- `src/auth.rs` — トークン管理（保存・読み込み・有効期限チェック）、setup-token フロー

### 変更
- `src/cli/mod.rs` — `Login`, `Logout` サブコマンド追加
- `src/main.rs` — login, logout コマンドのルーティング
- `src/cli/run.rs` — 認証フロー組み込み（トークン読み込み → 環境変数として渡す）
- `src/lib.rs` — `auth` モジュール追加
- `src/runtime/docker.rs` — `ContainerConfig` から credentials 関連フィールド削除

## テスト方針

### ユニットテスト
- トークンの保存・読み込み（パーミッション 600 確認含む）
- トークン有効期限チェック（expired / needs_renewal / valid）
- トークン削除

### 統合テスト
- `vibepod login` / `vibepod logout` の CLI パーステスト

### 手動テスト（TTY 操作が必要）
- `vibepod login` → ブラウザ自動起動 → 認可 → トークン保存
- `vibepod run` → 認証成功で Claude Code が使える
- `vibepod logout` → トークン削除確認
