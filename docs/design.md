# VibePod Design Spec v1

AI コーディングエージェントを Docker コンテナ内で安全に自律実行するための CLI ツール。

## 背景と課題

Claude Code の `--dangerously-skip-permissions` はすべての操作を自動承認し、自律実行を可能にする。Anthropic 公式はコンテナ内での使用を推奨しているが、安全に使うための手順が煩雑（Dockerfile 作成、docker run のオプション指定、マウント管理等）。

VibePod はこの手順を `init` + `run` の 2 コマンドに簡素化する。

## ゴール

- `vibepod init` でセットアップ完了
- `vibepod run` でコンテナ内自律実行開始
- 将来的に Claude Code 以外の AI Agent（Gemini CLI, Codex）にも対応
- OSS として公開し、AI 開発者コミュニティに提供

## 非ゴール（v1 スコープ外）

- ダッシュボード（Web UI での進捗確認）→ v2
- Gemini CLI / Codex 対応 → v2 以降
- 1Password CLI 連携 → v1.1 以降
- `vibepod restore`（git HEAD 自動リカバリ）→ v1.1 以降
- `--detach` によるバックグラウンド実行 → v2（ダッシュボードと合わせて実装）

---

## アーキテクチャ

### プロジェクト構成

```
vibepod/
├── src/
│   ├── main.rs           # エントリポイント
│   ├── cli/
│   │   ├── mod.rs        # clap によるコマンド定義
│   │   ├── init.rs       # vibepod init
│   │   └── run.rs        # vibepod run
│   ├── runtime/
│   │   └── docker.rs     # bollard による Docker API 操作
│   ├── config/
│   │   └── mod.rs        # 設定ファイルの読み書き
│   └── ui/
│       └── mod.rs        # dialoguer / indicatif による対話UI
├── templates/
│   └── Dockerfile        # バンドルする Dockerfile テンプレート
├── Cargo.toml
└── README.md
```

### 主要クレート

| クレート | 用途 |
|---------|------|
| `clap` | CLI 引数パース |
| `bollard` | Docker Engine API クライアント |
| `dialoguer` | 対話プロンプト（選択、確認） |
| `indicatif` | プログレスバー・スピナー |
| `tokio` | 非同期ランタイム（bollard が要求） |
| `serde` / `serde_json` | 設定ファイルのシリアライズ |

---

## コマンド詳細

### `vibepod init`

初回セットアップ。Docker イメージのビルドとグローバル設定の作成を行う。

**フロー：**

1. Docker Engine が動作しているか確認（bollard で接続テスト）
2. 使用する AI Agent を選択（v1 は Claude Code のみ、他は coming soon 表示）
3. パッケージにバンドルされた Dockerfile を使い Docker イメージをビルド
4. グローバル設定を保存

**Dockerfile（バンドル）：**

```dockerfile
FROM node:24-slim

ARG HOST_UID=501
ARG HOST_GID=20

RUN apt-get update && apt-get install -y --no-install-recommends \
  git sudo jq curl \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

# ホストの uid/gid に合わせて node ユーザーを再設定（macOS: 501:20）
# --non-unique で既存 GID との衝突を許容（macOS GID 20 は Debian の dialout と重複）
RUN groupmod --non-unique -g ${HOST_GID} node && \
    usermod --non-unique -u ${HOST_UID} -g ${HOST_GID} node && \
    chown -R node:node /home/node

RUN mkdir -p /usr/local/share/npm-global && \
    chown -R node:node /usr/local/share

ENV NPM_CONFIG_PREFIX=/usr/local/share/npm-global
ENV PATH=$PATH:/usr/local/share/npm-global/bin

USER node
ARG CLAUDE_VERSION=latest
RUN npm install -g @anthropic-ai/claude-code@${CLAUDE_VERSION}

USER root
RUN mkdir -p /workspace && chown node:node /workspace

USER node
WORKDIR /workspace

ENTRYPOINT ["claude"]
```

> **uid マッピング**: macOS のデフォルト uid は 501 だが、Linux は 1000 が一般的。
> `vibepod init` 時にホストの uid/gid を検出し、`--build-arg` で Dockerfile に渡す。
> これによりコンテナ内の `node` ユーザーとホストのファイル所有者が一致し、
> git 操作やファイル書き込みの権限問題を回避する。

**グローバル設定 (`~/.config/vibepod/config.json`)：**

```json
{
  "default_agent": "claude",
  "image": "vibepod-claude:latest",
  "claude_version": "latest"
}
```

> **バージョン管理**: デフォルトは `latest` で常に最新の Claude Code を使用する。
> 特定バージョンに固定したい場合は `vibepod init --claude-version X.Y.Z` で指定可能。
> 設定は `config.json` に保存され、次回の `vibepod init`（イメージ再ビルド）時に使用される。

**CLI 出力イメージ：**

```
 ██╗   ██╗██╗██████╗ ███████╗██████╗  ██████╗ ██████╗
 ██║   ██║██║██╔══██╗██╔════╝██╔══██╗██╔═══██╗██╔══██╗
 ██║   ██║██║██████╔╝█████╗  ██████╔╝██║   ██║██║  ██║
 ╚██╗ ██╔╝██║██╔══██╗██╔══╝  ██╔═══╝ ██║   ██║██║  ██║
  ╚████╔╝ ██║██████╔╝███████╗██║     ╚██████╔╝██████╔╝
   ╚═══╝  ╚═╝╚═════╝ ╚══════╝╚═╝      ╚═════╝ ╚═════╝

  ◇  Welcome to VibePod!
  │
  ◆  Which AI coding agent do you use?
  │  ● Claude Code
  │  ○ Gemini CLI (coming soon)
  │  ○ OpenAI Codex (coming soon)
  │
  ◇  Building Docker image: vibepod-claude...
  │  ██████████████████████████████ 100%
  │
  ◇  Done! Run `vibepod run` in any git repo to start.
  └
```

### `vibepod run`

カレントディレクトリの git リポジトリをコンテナ内で自律実行する。

**フロー：**

1. カレントディレクトリが git リポジトリか確認
2. `~/.config/vibepod/config.json` からイメージ名を取得
3. 初回実行のプロジェクトなら登録するか対話で確認
4. Docker API でコンテナを作成・起動
5. コンテナの stdout/stderr をターミナルにストリーミング
6. Ctrl+C でコンテナを停止・削除

**CLI オプション：**

| オプション | 説明 |
|-----------|------|
| `--resume` | 前回のセッションを引き継ぐ（Claude Code に渡す） |
| `--prompt "..."` | 初期プロンプトを指定（Claude Code に渡す） |
| `--no-network` | コンテナのネットワークを無効化する（`npm install` 等も不可になる点に注意） |
| `--env KEY=VALUE` | コンテナに環境変数を渡す（複数指定可。例: `--env ANTHROPIC_API_KEY=sk-...`） |

> **`--prompt` 未指定時**: Claude Code は `--dangerously-skip-permissions` + `--resume` で起動するため、
> 前回セッションの計画を引き継いで自律実行する。`--resume` なしかつ `--prompt` なしの場合は
> エラーを返す（意図しない対話モードでのコンテナ起動を防止）。

**コンテナ構成：**

マウントするもの：

| ホスト | コンテナ | モード | 目的 |
|--------|---------|--------|------|
| `$(pwd)` | `/workspace` | read-write | プロジェクトファイル |
| `~/.claude` | `/home/node/.claude` | **read-write** | 認証・プラグイン設定・セッション情報 |
| `~/.claude.json` | `/home/node/.claude.json` | read-only | オンボーディング状態 |

> **`~/.claude` が read-write の理由**: Claude Code はセッション情報（会話履歴）を
> `~/.claude/projects/` 配下に保存する。`--resume` でセッションを引き継ぐには
> この領域への書き込みが必要。`~/.claude.json` は設定の読み取りのみなので read-only を維持。

**認証情報の受け渡し：**

Claude Code は以下の優先順で認証情報を探す：
1. 環境変数 `ANTHROPIC_API_KEY`
2. `~/.claude` 内の OAuth トークン

VibePod は `~/.claude` のマウントにより OAuth トークンを共有する（デフォルト動作）。
API キーを使いたいユーザーは `--env ANTHROPIC_API_KEY=...` オプションで環境変数を渡せる。

マウントしないもの（セキュリティ）：

- `~/.ssh` — コンテナ内から SSH アクセスさせない
- `.env` — シークレットの漏洩防止
- ホームディレクトリ全体

**プロジェクト登録 (`~/.config/vibepod/projects.json`)：**

v2 のダッシュボードでプロジェクト一覧・実行履歴を表示するためのデータ。
v1 では登録・保存のみ行い、参照する UI は v2 で実装する。

```json
{
  "projects": [
    {
      "name": "my-project",
      "path": "/home/user/repos/my-project",
      "remote": "github.com/user/my-project",
      "registered_at": "2026-03-22T10:00:00Z"
    }
  ]
}
```

**CLI 出力イメージ：**

```
  ┌  VibePod
  │
  ◇  Detected git repository: my-project
  │  Remote: github.com/user/my-project
  │  Branch: feat/add-dashboard
  │
  ◆  First time running in this project. Register it?
  │  ● Yes, register and continue
  │  ○ No, one-time run
  │
  ◇  Starting container...
  │  Agent: Claude Code
  │  Mode: --dangerously-skip-permissions
  │  Mount: /home/user/repos/my-project → /workspace
  │
  ◇  Container started: vibepod-my-project-a1b2c3
  │  Press Ctrl+C to stop the container.
  └
```

---

## セキュリティモデル

### 3 層の隔離

```
┌─────────────────────────────────┐
│  ホスト OS                       │
│  ┌───────────────────────────┐  │
│  │  Docker コンテナ            │  │
│  │  ┌─────────────────────┐  │  │
│  │  │  Claude Code        │  │  │
│  │  │  (skip-permissions) │  │  │
│  │  └─────────────────────┘  │  │
│  │  見えるもの:               │  │
│  │   ✅ /workspace (プロジェクト) │
│  │   ✅ 認証情報 (~/.claude rw) │
│  │   ❌ ホストFS              │  │
│  │   ❌ ~/.ssh               │  │
│  │   ❌ 他のコンテナ           │  │
│  └───────────────────────────┘  │
└─────────────────────────────────┘
```

### 安全性の根拠

- **Anthropic 公式推奨**: `--dangerously-skip-permissions` はコンテナ内での使用を推奨
- **最小マウント**: プロジェクトディレクトリと認証情報のみ
- **認証情報の最小権限**: `~/.claude.json` は read-only、`~/.claude` はセッション永続化のため read-write だがプロジェクトスコープ外のホスト FS には触れない
- **git によるリカバリ**: プロジェクトは git 管理前提。壊れたら `git reset --hard` で復帰可能
- **コンテナ停止**: `Ctrl+C` または `docker stop` でいつでも停止可能

---

## エラーハンドリング

| 状況 | 挙動 |
|------|------|
| Docker Engine が起動していない | エラーメッセージ + `Docker Desktop / OrbStack を起動してください` と案内 |
| `vibepod init` 未実行で `run` | エラーメッセージ + `vibepod init を先に実行してください` と案内 |
| カレントディレクトリが git リポジトリでない | エラーメッセージ + `git init されたディレクトリで実行してください` と案内 |
| `~/.claude` が存在しない | エラーメッセージ + `claude を一度起動してログインしてください` と案内 |
| `--resume` も `--prompt` も未指定 | エラーメッセージ + どちらかの指定を求める |
| 同一プロジェクトで既にコンテナが実行中 | 既存コンテナに attach するか、停止して新規起動するか選択 |
| Docker イメージのビルド失敗（ネットワーク等） | エラー内容を表示 + リトライを促す |

## コンテナ管理

**命名規則**: `vibepod-{プロジェクト名}-{短縮ハッシュ(6桁)}`

例: `vibepod-my-project-a1b2c3`

- プロジェクト名は git リポジトリのディレクトリ名から取得
- 短縮ハッシュはコンテナ作成時にランダム生成

**シグナルハンドリング (Ctrl+C):**

1. SIGINT を受信
2. コンテナに SIGTERM を送信（Claude Code が graceful に終了する猶予）
3. 10 秒のタイムアウト
4. タイムアウト後、SIGKILL で強制停止
5. コンテナを削除

> **コンテナは ephemeral（使い捨て）**: コンテナ自体は毎回作成・削除される。
> セッション情報は `~/.claude` マウント経由でホスト側に永続化されるため、
> コンテナ削除後も `--resume` で前回セッションを引き継げる。

---

## 配布

### インストール方法

```bash
# macOS (Homebrew) — 推奨
brew install vibepod

# Linux / macOS (インストールスクリプト)
curl -fsSL https://vibepod.dev/install.sh | sh

# Rust ユーザー
cargo install vibepod
```

### `vp` エイリアス

`vibepod` バイナリが `argv[0]` を確認し、`vp` として呼ばれても同じ動作をする。インストール時にシンボリックリンクとして `vp` を配置する。

### CI / リリース

- GitHub Actions で macOS (`x86_64`, `aarch64`) / Linux (`x86_64`, `aarch64`) のバイナリを自動ビルド
- GitHub Releases に配置
- Homebrew tap (`vibepod/tap`) が GitHub Releases を参照

---

## ロードマップ

| バージョン | 機能 |
|-----------|------|
| **v1** | `init` + `run`、Claude Code 対応、Homebrew 配布 |
| **v1.1** | 1Password CLI 連携、`vibepod restore`（git HEAD 自動リカバリ） |
| **v2** | ダッシュボード（Web UI）、実行ログ、進捗モニタリング |
| **v2.1+** | Gemini CLI / Codex 対応、マルチコンテナ同時実行 |
