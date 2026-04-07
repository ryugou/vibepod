# ホスト Claude 環境のコンテナ取り込み 設計仕様

## 概要

ユーザーがホスト（macOS/Linux）で使っている Claude Code のプラグイン・設定を、vibepod コンテナ内でもそのまま使えるようにする。特に `codex` プラグインのように、ホストでは利用可能だがコンテナ内で利用不可だった plugin を取り込めるようにする。

## 背景と問題

### 現状

- Dockerfile で `superpowers` と `frontend-design` を全ユーザー共通で焼き込んでいる（`templates/Dockerfile:23-27`）
- ランタイムでは `~/.claude/{CLAUDE.md,skills,agents}` のみを `/home/vibepod/.claude/...` にマウント（`src/cli/run/mod.rs:125-154` の `build_claude_config_mounts`）
- `~/.claude/plugins/` は一切マウントされていない
- `~/.claude/settings.json` も一切マウントされていない

### 結果として起きている問題

1. ホストで使っている `codex` プラグインがコンテナ内で利用不可 → `/codex:review` などのコマンドが使えない
2. ホストで使っている任意のプラグイン・skill の利用状態がコンテナに引き継がれない
3. 結果、コンテナ内の Claude Code がホストと異なる品質・挙動になる

## 過去の制約（既知の失敗）

`docs/superpowers/specs/2026-03-23-vibepod-auth-design.md:9-12` に記録：

> `~/.claude/` 全体をマウントすると、プラグインキャッシュ等の読み込みで Claude Code がハングする

この失敗の構造は「Claude Code の HOME ディレクトリ `/home/vibepod/.claude/` 全体が bind mount 上に載り、session 書き込み/cache/lock/telemetry の I/O が全て bind mount 越しになる」ことだったと推定する。本仕様は「必要なサブディレクトリのみを ro マウントし、HOME はネイティブ FS のままにする」ため、この失敗パターンには該当しない。

## 設計方針

### 原則

- **ホスト側の設定は一切変更しない**。vibepod が書き込むのは `~/.config/vibepod/` 配下のみ
- **必要なサブディレクトリのみを ro マウント**する
- **`~/.claude/` 全マウントは禁止**（過去事例より）
- **Dockerfile は変更しない**（baked plugins はホストに plugins が無い場合のフォールバックとして残す）

### マウント対象

| 対象 | ホスト側 | コンテナ側 | 備考 |
|---|---|---|---|
| CLAUDE.md | `~/.claude/CLAUDE.md` | `/home/vibepod/.claude/CLAUDE.md` | **既存** |
| skills | `~/.claude/skills/` | `/home/vibepod/.claude/skills` | **既存** |
| agents | `~/.claude/agents/` | `/home/vibepod/.claude/agents` | **既存** |
| plugins (1/2) | `~/.claude/plugins/` | `/home/vibepod/.claude/plugins` | **新規**。Claude Code が `$HOME/.claude/plugins/installed_plugins.json` を読む先 |
| plugins (2/2) | `~/.claude/plugins/` | `<host_home>/.claude/plugins` | **新規**。`installed_plugins.json` 内のホスト絶対パス `installPath` を解決するための二重マウント |
| settings.json | `~/.config/vibepod/runtime/<container>/settings.json`（vibepod が生成） | `/home/vibepod/.claude/settings.json` | **新規**。ホスト原本を sanitize したコピーをマウント |

### plugins の二重マウントの根拠

`~/.claude/plugins/installed_plugins.json` には `installPath` フィールドがあり、ホスト絶対パス（例: `/Users/ryugo/.claude/plugins/cache/openai-codex/codex/1.0.1`）が埋まっている。Claude Code がこの絶対パスを追って plugin 本体を読む際、コンテナ内に該当パスが存在する必要がある。

解決策として以下の 2 つの bind mount を作る：

```
-v ~/.claude/plugins:/home/vibepod/.claude/plugins:ro   # $HOME 経由の読み先
-v ~/.claude/plugins:<host_home>/.claude/plugins:ro     # 絶対パス経由の読み先
```

両方とも同じホストデータを指すため整合性は自動的に保たれる。symlink は不要。

`<host_home>` は実行時に `HOME` 環境変数から決定する（macOS: `/Users/<user>`, Linux: `/home/<user>`）。

### settings.json のサニタイズ

ホストの `~/.claude/settings.json` には以下のようなホスト固有フィールドが含まれる：

- `hooks`: 絶対パスでホストスクリプトを参照（例: `/Users/ryugo/.claude/hooks/cc-slack-notify.sh`）
- `statusLine`: 同様にホストスクリプトを参照する可能性

これらをコンテナに持ち込むと、パス解決失敗・意図しない副作用のリスクがある。vibepod は以下を行う：

1. ホスト `~/.claude/settings.json` を読む
2. `hooks` と `statusLine` フィールドを除去した JSON を生成
3. `~/.config/vibepod/runtime/<container-name>/settings.json` に書き出す
4. 上記ファイルを `/home/vibepod/.claude/settings.json` に ro マウント

残すフィールド（ホワイトリストではなく、ブラックリスト方式で除去）：
- `env`, `permissions`, `enabledPlugins`, `extraKnownMarketplaces`, `teammateMode`, その他ホスト絶対パスを含まないもの

### ~/.claude/plugins が存在しない場合の動作

- `build_claude_config_mounts` はすでに存在チェックを行っており、該当ディレクトリが無ければマウントしない
- 同じパターンを plugins にも適用する
- マウントが無い場合、Dockerfile で baked された `superpowers`/`frontend-design` がコンテナ内で有効なまま（既存の挙動）

### ~/.claude/settings.json が存在しない場合の動作

- サニタイズ処理をスキップし、マウントしない
- Dockerfile で baked された設定があればそれが使われる

## 影響範囲

### 変更対象ファイル

| 種別 | パス | 責務 |
|---|---|---|
| 変更 | `src/cli/run/mod.rs` | `build_claude_config_mounts` に plugins 2エントリ追加、`sanitize_settings_json` 関数追加 |
| 変更 | `src/cli/run/prepare.rs` | サニタイズ済み settings.json の生成と extra_mounts への追加 |
| 変更 | `tests/run_logic_test.rs` | 新規マウント・サニタイズのユニットテスト |
| 変更 | `README.md` | Security Model のマウント一覧更新 |
| 変更 | `docs/design.md` | マウントの章更新 |

### 変更しないもの

- `templates/Dockerfile`（baked plugins はフォールバック用に残す）
- `src/runtime/docker.rs`（既存の `extra_mounts` 仕組みに乗るだけ）
- auth 関連（変更不要）

## テスト方針

### ユニットテスト（`tests/run_logic_test.rs`）

- `build_claude_config_mounts` に plugins ディレクトリがある場合、2 エントリ（`/home/vibepod/.claude/plugins` と `<host_home>/.claude/plugins`）が返る
- plugins ディレクトリが無い場合、該当エントリは返らない
- `sanitize_settings_json`: `hooks` と `statusLine` が除去される
- `sanitize_settings_json`: それ以外のフィールド（`env`, `permissions`, `enabledPlugins` 等）は保持される
- `sanitize_settings_json`: 入力が空 object の場合、空 object が返る

### 手動 E2E 検証

1. `vibepod init` でイメージをビルド（Dockerfile 変更なしなので既存イメージ再利用可）
2. `vibepod run --new` で新規コンテナ起動（マウントが反映される）
3. コンテナ内で `claude` 起動 → ハングしないこと
4. コンテナ内で `/codex:review` コマンドが（ユーザー操作で）認識されること
5. コンテナ内で skills/agents/CLAUDE.md が既存挙動のまま動作すること
6. ホスト側の `~/.claude/plugins/` の中身が変更されていないこと（ro 確認）

## セキュリティ考慮

- すべてのマウントは **read-only**
- ホスト側の書き込みは `~/.config/vibepod/runtime/<container>/settings.json` のみ（vibepod 生成の sanitize コピー）
- 既存の `~/.claude.json` 温コピー方式（`src/cli/run/prepare.rs:509-518`）と同じ安全モデル
- `settings.json` の hooks 除去により、コンテナ内で意図しないホストスクリプトが起動されることは無い

## リリース

- v1.5 スコープに追加（v1.5.0 未リリース）
- ブレイキング変更ではない（既存ユーザーの挙動は維持、ホストに `~/.claude/plugins/` があればそれが優先される）
- README の Security Model 節のマウント一覧を更新
