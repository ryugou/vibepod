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
- `--detach` によるバックグラウンド実行 → v2（ダッシュボードと合わせて実装）

## 実装済み（v1.1 → v1.2）

- 1Password CLI 連携（`--env-file` の `op://` 参照解決）→ v1.1
- `vibepod restore`（git HEAD 自動リカバリ）→ v1.2
- `vibepod login` / `vibepod logout`（コンテナ独立認証）→ v1.2

---

## アーキテクチャ

### プロジェクト構成

```
vibepod/
├── src/
│   ├── main.rs           # エントリポイント
│   ├── lib.rs            # ライブラリクレート（モジュール公開）
│   ├── cli/
│   │   ├── mod.rs        # clap によるコマンド定義
│   │   ├── init.rs       # vibepod init
│   │   └── run.rs        # vibepod run
│   ├── runtime/
│   │   └── docker.rs     # bollard による Docker API 操作
│   ├── config/
│   │   └── mod.rs        # 設定ファイルの読み書き
│   └── ui/
│       └── mod.rs        # dialoguer による対話UI
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
FROM debian:bookworm-slim

ARG HOST_UID=501
ARG HOST_GID=20

RUN apt-get update && apt-get install -y --no-install-recommends \
  git sudo jq curl ca-certificates \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

# vibepod ユーザーを作成し、ホストの uid/gid に合わせる
RUN groupadd --non-unique -g ${HOST_GID} vibepod && \
    useradd -m -u ${HOST_UID} -g ${HOST_GID} -s /bin/bash vibepod

USER vibepod
ENV PATH=/home/vibepod/.local/bin:$PATH
RUN curl -fsSL https://claude.ai/install.sh | bash

# Claude Code プラグインのインストール
RUN claude plugin marketplace add anthropics/claude-code --sparse .claude-plugin plugins && \
    claude plugin marketplace add obra/superpowers-marketplace && \
    claude plugin install superpowers && \
    claude plugin install frontend-design

USER root
RUN mkdir -p /workspace && chown vibepod:vibepod /workspace

USER vibepod
WORKDIR /workspace

CMD ["claude"]
```

> **uid マッピング**: macOS のデフォルト uid は 501 だが、Linux は 1000 が一般的。
> `vibepod init` 時にホストの uid/gid を検出し、`--build-arg` で Dockerfile に渡す。
> これによりコンテナ内の `vibepod` ユーザーとホストのファイル所有者が一致し、
> git 操作やファイル書き込みの権限問題を回避する。

**グローバル設定 (`~/.config/vibepod/config.json`)：**

```json
{
  "default_agent": "claude",
  "image": "vibepod-claude:latest"
}
```

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
| `--prompt "..."` | 初期プロンプトを指定（Claude Code に `-p` フラグとして渡す） |
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
| `~/.claude.json` | `/home/vibepod/.claude.json` | read-write | オンボーディング状態 |
| `~/.gitconfig` | `/home/vibepod/.gitconfig` | read-only | git ユーザー情報（name, email） |

> **`~/.claude` をマウントしない理由**: `~/.claude` 全体をマウントすると、プラグインキャッシュ等の
> 読み込みで Claude Code がハングする。認証は環境変数で渡すため、マウント不要。

**認証情報の受け渡し：**

`vibepod login` で `claude setup-token` を実行し、1 年有効の長期 OAuth トークンを取得する。
このトークンは `~/.config/vibepod/auth/token.json` に保存され、`vibepod run` 時に
環境変数 `CLAUDE_CODE_OAUTH_TOKEN` としてコンテナに渡される。

- ホスト側の OAuth セッションとは独立したトークン（競合しない）
- 複数コンテナで同じトークンを同時使用可能
- credentials ファイルのマウント不要

マウントしないもの（セキュリティ）：

- `~/.ssh` — コンテナ内から SSH アクセスさせない
- `~/.claude` — プラグインキャッシュでハングするため。認証は環境変数で代替
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
│  │   ✅ 認証トークン (環境変数)   │
│  │   ✅ ~/.gitconfig (ro)      │  │
│  │   ❌ ホストFS              │  │
│  │   ❌ ~/.ssh               │  │
│  │   ❌ ~/.claude             │  │
│  │   ❌ 他のコンテナ           │  │
│  └───────────────────────────┘  │
└─────────────────────────────────┘
```

### 安全性の根拠

- **Anthropic 公式推奨**: `--dangerously-skip-permissions` はコンテナ内での使用を推奨
- **最小マウント**: プロジェクトディレクトリ、`.claude.json`、`.gitconfig` のみ
- **認証情報の分離**: OAuth トークンは環境変数で渡す。`~/.claude` はマウントしない（ハングの原因になるため）
- **git によるリカバリ**: プロジェクトは git 管理前提。壊れたら `git reset --hard` で復帰可能
- **コンテナ停止**: `Ctrl+C` または `docker stop` でいつでも停止可能

---

## エラーハンドリング

| 状況 | 挙動 |
|------|------|
| Docker Engine が起動していない | エラーメッセージ + `Docker Desktop / OrbStack を起動してください` と案内 |
| `vibepod init` 未実行で `run` | エラーメッセージ + `vibepod init を先に実行してください` と案内 |
| `vibepod login` 未実行で `run` | エラーメッセージ + `vibepod login を先に実行してください` と案内 |
| トークンの有効期限が残り 7 日以内 | `vibepod login` を再実行してくださいと案内（強制） |
| カレントディレクトリが git リポジトリでない | エラーメッセージ + `git init されたディレクトリで実行してください` と案内 |
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
> `~/.claude` はマウントしないため、コンテナ内のセッション情報は永続化されない。
> `--resume` でのセッション引き継ぎには制限がある。

---

## 配布

### インストール方法

```bash
# macOS (Homebrew)
brew tap ryugou/tap
brew install vibepod

# Linux / macOS (インストールスクリプト)
curl -fsSL https://raw.githubusercontent.com/ryugou/vibepod/main/install.sh | sh

# Rust ユーザー
cargo install vibepod
```

### `vp` エイリアス

インストール時にシンボリックリンクとして `vp` を配置する。`vibepod` と同一のバイナリを指す。

### CI / リリース

- GitHub Actions で macOS (`x86_64`, `aarch64`) / Linux (`x86_64`, `aarch64`) のバイナリを自動ビルド
- GitHub Releases に配置
- Homebrew tap (`ryugou/homebrew-tap`) が GitHub Releases を参照

