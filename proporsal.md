Docker コンテナ内で Claude Code を自律実行し、寝ている間に実装を完了させるための手順。

---

## 前提

- 開発機: MacBook Pro M5 Pro（OrbStack + Docker 導入済み）
- Claude Code インストール済み（ホスト側は対話用）
- GitHub にリポジトリ作成済み

---

## Phase 0: 初回セットアップ（一度だけ）

### 0-1. devcontainer の準備

Anthropic 公式リファレンスをベースに、プロジェクト横断で使える devcontainer を用意する。

```bash
mkdir -p ~/Developer/src/settings/claude-devcontainer/.devcontainer
cd ~/Developer/src/settings/claude-devcontainer
```

`.devcontainer/Dockerfile` を作成：

```docker
FROM node:24-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
  git \
  sudo \
  iptables \
  ipset \
  iproute2 \
  dnsutils \
  jq \
  curl \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /usr/local/share/npm-global && \
    chown -R node:node /usr/local/share

ENV NPM_CONFIG_PREFIX=/usr/local/share/npm-global
ENV PATH=$PATH:/usr/local/share/npm-global/bin

USER node

# Claude Code インストール
RUN npm install -g @anthropic-ai/claude-code

USER root
RUN mkdir -p /workspace && chown node:node /workspace

USER node
WORKDIR /workspace

ENTRYPOINT ["claude"]
```

イメージをビルド：

```bash
docker build -t claude-code-runner .devcontainer/
```

### 0-2. プラグインのインストール

ホスト側で Claude Code を起動してプラグインを入れる（user scope）：

```bash
claude
```

セッション内で以下を実行：

```
/plugin marketplace add anthropics/claude-code
/plugin install frontend-design@anthropics-claude-code

/plugin marketplace add obra/superpowers-marketplace
/plugin install superpowers@superpowers-marketplace

/plugin list
```

確認後 `Ctrl+C` で終了。

### 0-3. ディレクトリ構造

```
cp -r ~/Developer/src/settings/claude-devcontainer ~/Developer/src/AI-Project/

~/Developer/src/AI-Project/open-claw-agent/
├── claude-devcontainer/       ← devcontainer 定義（上で作成）
│   └── .devcontainer/
│       └── Dockerfile
├── ai-development/            ← マルチリポジトリ作業場
│   ├── .claude/
│   ├── multi-agent-orchestrator/
│   └── openclaw-skills/
├── auto-post-dashboard/       ← 今回の新規プロジェクト
│   └── .claude/
└── ...
```

---

## Phase 1: プロジェクト初期化（プロジェクトごとに1回）

### 1-1. リポジトリのクローンと .claude/ 作成

```bash
cd ~/Developer/src/AI-Project/open-claw-agent
git clone git@github.com:ryugou/auto-post-dashboard.git
cd auto-post-dashboard
mkdir -p .claude/specs
```

### 1-2. プロジェクト CLAUDE.md を作成

`.claude/CLAUDE.md` に最低限のルールを書く：

```markdown
# auto-post-dashboard

## 概要
自動投稿エージェントのダッシュボード

## 技術スタック
（Phase 2 のブレストで決まったら埋める）

## ルール
- グローバルルール（~/.claude/CLAUDE.md）に従う
- Docker-only で開発する（ホスト直接実行しない）
```

### 1-3. Git の checkpoint

```bash
git add -A
git commit -m "chore: initial claude code setup"
```

---

## Phase 2: ブレスト → 設計 → 計画（対話モード / ホスト）

ホスト側でプロジェクトディレクトリから claude を起動する：

```bash
cd ~/Developer/src/AI-Project/open-claw-agent/auto-post-dashboard
claude
```

Superpowers が自動発動する。普段 Web UI でやっていた壁打ちと同じことをやる。

### 2-1. ブレスト

```
自動投稿エージェントのダッシュボードを作りたい。
投稿スケジュールの管理、投稿状況のモニタリング、
エージェントの設定ができるようにしたい。
```

Claude が質問してくるので対話しながら要件を固める：

- 対象 SNS
- 認証方式
- 技術スタック
- デプロイ先

### 2-2. 設計の承認

Superpowers がセクションごとに設計を提示する。
確認して「OK」「この部分は修正して」と返す。

設計が fix したら `.claude/CLAUDE.md` の技術スタックを埋めるよう指示する。

### 2-3. 実装計画の作成

```
/superpowers:write-plan
```

2〜5分単位のタスクに分解された計画が生成される。
ファイルパス、テスト方針、検証ステップまで含まれる。

計画を確認し、承認する。

### 2-4. セッションの終了

計画が承認されたら `Ctrl+C` でセッションを終了する。

### 2-5. checkpoint を取る

```bash
git add -A
git commit -m "docs: design and implementation plan"
```

ここまでが「起きている間」の作業。

---

## Phase 3: 自律実行（Docker コンテナ / 放置モード）

### 3-1. Docker コンテナで Claude Code を起動

```bash
cd ~/Developer/src/AI-Project/open-claw-agent/auto-post-dashboard

docker run -it --rm \
  --name claude-builder \
  -v "$(pwd)":/workspace \
  -v ~/.claude:/home/node/.claude \
  -v ~/.claude.json:/home/node/.claude.json \
  claude-code-runner \
  --dangerously-skip-permissions \
  --resume
```

各オプションの意味：

| オプション | 意味 |
| --- | --- |
| `-v "$(pwd)":/workspace` | リポジトリをコンテナにマウント |
| `-v ~/.claude:/home/node/.claude` | 認証情報・プラグイン設定を共有 |
| `-v ~/.claude.json:/home/node/.claude.json` | オンボーディング状態を共有 |
| `--dangerously-skip-permissions` | 全操作を自動承認 |
| `--resume` | Phase 2 のセッション（設計・計画）を引き継ぐ |

コンテナ内で Claude Code が起動し、計画に従って自律的に実装を進める。

### 3-2. 放置

ターミナルはそのまま開いておく（画面を閉じない）。
Mac のスリープ設定を確認しておくこと（`caffeinate` コマンドで防止可能）：

```bash
# 別ターミナルで実行しておく
caffeinate -dims
```

### 3-3. 安全策

万が一おかしなことが起きた場合の保険：

- **Git**: Phase 2 で checkpoint を取っているので `git reset --hard HEAD` でリカバリ可能
- **コンテナ**: `docker stop claude-builder` でいつでも停止可能
- **ホスト影響なし**: volume mount されたディレクトリ以外には触れない

---

## Phase 4: 翌朝の確認

### 4-1. 実装結果の確認

```bash
cd ~/repos/auto-post-dashboard

# 何が変わったか確認
git log --oneline
git diff main
```

### 4-2. 対話モードでレビュー

```bash
claude
```

```
昨夜の実装結果をレビューして。
計画通りに進んだか、テストは通っているか確認して。
```

### 4-3. 修正があれば対応

問題があれば対話モードで修正を指示する。
大きな修正が必要なら、再度 Phase 3（Docker で自律実行）に戻す。

### 4-4. PR を作成

```
フィーチャーブランチを作成して PR を出して
```

---

## 日常の流れ（まとめ）

| 時間帯 | やること | 実行場所 |
| --- | --- | --- |
| 夕方 | ブレスト → 設計 → 計画承認 | ホスト `claude` |
| 寝る前 | checkpoint → Docker で自律実行開始 | `docker run ... --resume` |
| 翌朝 | レビュー → 修正 → PR | ホスト `claude` |

---

## 既存プロジェクトへの適用

設計ドキュメントが存在しないプロジェクトで Superpowers を使うと、ゼロからブレストが始まる。
これを避けるには、一度だけ以下を実行する：

```bash
cd ~/repos/既存プロジェクト
claude
```

```
現状のアーキテクチャと設計方針をドキュメント化して。
docs/architecture.md に保存して。
```

このドキュメントがあれば、次回以降は機能追加時にブレストではなく計画フェーズから始められる。

---

## 注意事項

- `-dangerously-skip-permissions` は必ず Docker コンテナ内で使う。ホストでは絶対に使わない
- コンテナに mount するのはプロジェクトディレクトリと Claude 認証情報のみ。`~/.ssh` や `.env` は mount しない
- `-resume` が前回セッションを見つけられない場合は、Docker 内で新規セッションが始まる。その場合は設計ドキュメントがリポジトリ内にあることが前提となる
