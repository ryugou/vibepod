# コンテナイメージへの Node.js 追加(案A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** vibepod コンテナの base ステージに Node.js を焼き込み、`codex-companion.mjs`(codex plugin の rescue サブエージェントが使う thin wrapper)がコンテナ内で実行できるようにする(Issue #65 案A)。

**Architecture:** `templates/Dockerfile` の base ステージに、既存の codex CLI ブロックと全く同じ方式(バージョン pin + アーキ別 SHA256 テーブル + ダウンロード後検証 + `mktemp -d` の一時ディレクトリ)で Node.js tarball を導入する 1 ブロックを追加する。base ステージに置くため default / swift 両 profile に伝播する。

**Tech Stack:** Dockerfile(Debian ベース、`dpkg --print-architecture` によるアーキ分岐、`curl` + `sha256sum -c` + `tar`)。

## Global Constraints

- 変更対象は `templates/Dockerfile` のみ(README に同梱ツールの一覧記載があればそこも追随)。CHANGELOG・バージョン bump は対象外。
- `ARG NODE_VERSION=22.23.2` を使う。
- 取得元: `https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-<arch>.tar.gz`(`arm64`→`arm64`、`amd64`→`x64`。それ以外は `exit 1`)。
- SHA256(アーキ別、コメントでどちらがどちらか明記):
  - `node-v22.23.2-linux-arm64.tar.gz`: `013b59cfd2819703a6f4a14ab891fc46fc2a4e3f5bcd92de3fb4929b43e35b30`
  - `node-v22.23.2-linux-x64.tar.gz`: `b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a`
- ダウンロード後 `sha256sum -c -` で検証してからでないと展開しない。`latest` エスケープハッチは設けない(未知の version/arch はビルド失敗)。
- 展開先は `/usr/local` へ `--strip-components=1`。一時ディレクトリは `mktemp -d` を使い処理後に `rm -rf`。
- 検証は `RUN node --version && npm --version` を別 RUN として置く。
- `.tar.gz` を使う(`.tar.xz` は `xz-utils` 追加が必要になり base の apt パッケージリスト変更を招くため不可)。
- 配置位置: base ステージの `RUN codex --version`(現 87 行目)の直後、`USER vibepod`(現 89 行目)の前。profile 固有ステージ(`profile-default` / `profile-swift`)には手を加えない。
- Dockerfile 内コメントに、(1) node を入れる理由(codex CLI 本体は musl 静的バイナリで node 不要だが、codex plugin の `codex-companion.mjs` が node 実行を必須としレビューフローをコンテナ内で完結させるために必要、Issue #65)、(2) 更新手順(`ARG` と SHA256 テーブルの両方を bump してから `vibepod init --rebuild`)を必ず含める。
- ビルド検証(`docker build` / `vibepod init --rebuild`)は実施しない(コンテナ内からホストの Docker は操作できないため、ホスト側の受理判断者が行う)。
- コミットは 1 つに、Conventional Commits 準拠、本文に Issue #65 への参照を含める。`git add` は変更ファイルを個別指定(`git add -A` / `git add .` 禁止)。push・PR 作成は禁止。

---

### Task 1: Dockerfile への Node.js 導入ブロック追加 + README 追随

**Files:**
- Modify: `templates/Dockerfile:86-89`(`RUN codex --version` の直後、`USER vibepod` の前に新ブロックを挿入)
- Modify: `README.md:185-187`(codex 節の「no node/npm required」という記述が、イメージに Node.js を同梱する今回の変更と矛盾するため追随修正)

**Interfaces:**
- 消費: 既存の codex ブロック(`templates/Dockerfile:59-87`)のパターン(`set -eu` → arch 判定 → asset/URL 組み立て → `case "${VERSION}:${arch}"` での SHA256 決定 → `mktemp -d` → `curl -fsSL` → `sha256sum -c -` → 展開 → `rm -rf`)をそのまま踏襲する。
- 産出: `/usr/local/bin/node`、`/usr/local/bin/npm` が PATH 上で使えるようになる(`ENV PATH` の追加変更は不要 — `/usr/local/bin` は base イメージで既に PATH に含まれる)。

- [ ] **Step 1: Dockerfile に Node.js 導入ブロックを追加する**

`templates/Dockerfile` の 87 行目(`RUN codex --version`)と 89 行目(`USER vibepod`)の間に、88 行目として以下を挿入する:

```dockerfile

# Node.js(codex CLI 本体は musl 静的バイナリで node を必要としないが、
# Claude Code の codex plugin が使う codex-companion.mjs は node 実行を
# 必須とし、レビューフローをコンテナ内で完結させるために導入する。
# Issue #65)。
#
# codex CLI と同じバージョン pin + SHA256 テーブル方式を採用する。
# latest エスケープハッチは設けない(未知の version/arch はビルド失敗
# として扱う)。更新は ARG と下記 SHA256 テーブルの両方を bump してから
# `vibepod init --rebuild` で行う(README 参照)。
ARG NODE_VERSION=22.23.2

# pin されたバージョンごとの SHA256(アーキ別)。NODE_VERSION を bump する際は
# 対応する行をこのテーブルに追加すること。未知の組み合わせはビルド失敗として
# 扱う(取り違え防止のため、どちらのハッシュがどちらのアーキ用かを下記コメン
# トで明記する)。
#   node-v22.23.2-linux-arm64.tar.gz (arm64) の SHA256:
#     013b59cfd2819703a6f4a14ab891fc46fc2a4e3f5bcd92de3fb4929b43e35b30
#   node-v22.23.2-linux-x64.tar.gz (amd64) の SHA256:
#     b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      arm64) node_arch=arm64 ;; \
      amd64) node_arch=x64 ;; \
      *) echo "node: unsupported architecture '$arch'" >&2; exit 1 ;; \
    esac; \
    asset="node-v${NODE_VERSION}-linux-${node_arch}.tar.gz"; \
    case "${NODE_VERSION}:${node_arch}" in \
      "22.23.2:arm64") expected_sha256="013b59cfd2819703a6f4a14ab891fc46fc2a4e3f5bcd92de3fb4929b43e35b30" ;; \
      "22.23.2:x64") expected_sha256="b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a" ;; \
      *) echo "node: no known SHA256 for NODE_VERSION=${NODE_VERSION} (arch ${node_arch}); bump the checksum table in this Dockerfile" >&2; exit 1 ;; \
    esac; \
    url="https://nodejs.org/dist/v${NODE_VERSION}/${asset}"; \
    tmp_dir="$(mktemp -d)"; \
    curl -fsSL "$url" -o "${tmp_dir}/${asset}"; \
    echo "${expected_sha256}  ${tmp_dir}/${asset}" | sha256sum -c -; \
    tar -xzf "${tmp_dir}/${asset}" -C /usr/local --strip-components=1; \
    rm -rf "${tmp_dir}"
RUN node --version && npm --version
```

挿入後、ファイル全体を見て以下を目視確認する:
- 新ブロックが `USER root` のコンテキストのまま実行される(35 行目 `USER root` から 89 行目 `USER vibepod` の間に挿入されている)こと。
- `profile-default` / `profile-swift` ステージ(96 行目以降)には一切変更が無いこと。

- [ ] **Step 2: 挿入した RUN ブロックのシェル構文を静的チェックする**

Docker ビルドは行わない(ホスト Docker を操作できないため)。代わりに `RUN` 内のシェルスクリプト部分だけを抜き出し、構文エラーが無いことを `sh -n` で確認する:

```bash
awk '/^RUN set -eu; \\$/,/rm -rf "\$\{tmp_dir\}"$/' templates/Dockerfile | sed 's/\\$//' | sh -n
```

Expected: 何も出力されず、終了コード 0(構文エラーなし)。

- [ ] **Step 3: SHA256 値と ARG の転記ミスが無いことを確認する**

```bash
grep -n "NODE_VERSION\|013b59cfd2819703a6f4a14ab891fc46fc2a4e3f5bcd92de3fb4929b43e35b30\|b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a" templates/Dockerfile
```

Expected: `ARG NODE_VERSION=22.23.2` の行と、2 つの SHA256 値がそれぞれ 1 回ずつコメントと `case` 文中に(計 2 回ずつ)出現する。

- [ ] **Step 4: README の codex 節を Node.js 同梱の事実に合わせて更新する**

`README.md:185-187` の現在の文面:

```markdown
The container image bundles the `codex` CLI (musl static binary, no node/npm
required) so the implementation-delegation flow can run a `codex` review
before code leaves the container.
```

を、以下に置き換える:

```markdown
The container image bundles the `codex` CLI (musl static binary; the `codex`
binary itself does not need node/npm) so the implementation-delegation flow
can run a `codex` review before code leaves the container. The image also
bundles Node.js, because the Codex Claude Code plugin's review path shells
out to `codex-companion.mjs`, which does require a node runtime.
```

- [ ] **Step 5: README の変更を目視確認する**

```bash
grep -n "no node/npm\|does not need node/npm\|bundles Node.js" README.md
```

Expected: 「no node/npm required」という古い文言が消え、「does not need node/npm」と「bundles Node.js」の 2 行がヒットする。

- [ ] **Step 6: フォーマット・lint(Rust コード変更は無いが CLAUDE.md のコミット前チェックリストに従い確認)**

このタスクは `templates/Dockerfile` と `README.md` のみが対象で Rust コードは変更しないため、`cargo fmt` / `cargo clippy` の対象差分は無い。念のため差分に `.rs` ファイルが含まれていないことを確認する:

```bash
git status --porcelain | grep -v '\.rs$'
```

Expected: 変更ファイルは `templates/Dockerfile` と `README.md` のみ(`.rs` ファイルは 1 件もヒットしない = grep 結果に `.rs$` 行が無い)。

- [ ] **Step 7: コミット**

```bash
git add templates/Dockerfile README.md
git commit -m "feat(container): bundle Node.js in base image for codex-companion.mjs

codex CLI itself is a musl static binary and does not need node/npm, but
the Codex Claude Code plugin's rescue subagent shells out to
codex-companion.mjs via node, which the container previously lacked. This
blocked the codex review step of the in-container review flow entirely.

Refs #65"
```
