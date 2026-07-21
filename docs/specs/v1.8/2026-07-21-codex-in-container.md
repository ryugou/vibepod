# v1.8: コンテナ内 codex レビュー対応(codex CLI 同梱 + 認証注入)

## 背景・目的

vibepod の実装委譲フロー(実装→セルフレビュー)に codex レビューまで含め、
「コンテナから出てくる成果物は codex レビュー PASS 済み」を保証できるようにする。
現状コンテナには codex CLI が存在せず、OpenAI 認証の注入経路もないため、
コンテナ内で code-review スキル(`codex:codex-rescue` サブエージェント)を実行すると失敗する。

## 要件

### 1. codex CLI のイメージ同梱(`templates/Dockerfile`)

- openai/codex の GitHub リリースから **musl 静的バイナリ**を取得して `/usr/local/bin/codex` に配置する
  - アセット名: `codex-aarch64-unknown-linux-musl.tar.gz` / `codex-x86_64-unknown-linux-musl.tar.gz`
  - URL 形式: `https://github.com/openai/codex/releases/latest/download/<asset>`
  - ビルドアーキテクチャに応じて選択する(`dpkg --print-architecture`: arm64→aarch64, amd64→x86_64)
- node / npm は導入しない(バイナリ直置きで足りる)
- ビルド時に `codex --version` を実行して導入を検証する(失敗したらビルドを落とす)
- インストール後の更新は `vibepod init --rebuild` で行う(claude のような実行時自動更新は本バージョンでは対象外。README に明記)

### 2. codex 認証・設定の注入(`src/cli/run/prepare.rs` ほか)

- ホストの `~/.codex/auth.json` と `~/.codex/config.toml` の **2ファイルのみ**を対象とする(allowlist 方式)
  - `history.jsonl` / `goals_*.sqlite` / `cache/` 等は機微データ・不要データのため**絶対に持ち込まない**
- 既存の `.claude.json` と同じ **copy-then-mount** パターンを踏襲する:
  - `<runtime_dir>/codex/` にコピー(ディレクトリ 0700、ファイル 0600)し、`/home/vibepod/.codex` にマウント
  - codex はトークンリフレッシュ時に `auth.json` を書き換えるため、マウントは **read-write**(コピーなのでホスト原本は影響を受けない)
- `auth.json` が存在しない場合はエラーにせずスキップし、
  「codex auth not found (~/.codex/auth.json); codex review is unavailable in this container」と
  stderr に1行出す(エラーの握りつぶし禁止に従い、無言スキップは不可)
- `config.toml` のみ存在しない場合は auth.json だけ注入して続行(警告不要)

### 3. テスト

以下を純関数化してユニットテストすること(既存 `tests/host_claude_assets.rs` の流儀に合わせる):
- codex 注入対象の列挙ロジック: auth.json + config.toml のみが対象になること、
  `history.jsonl` 等が决して含まれないこと(allowlist の検証)
- auth.json 欠如時にマウントエントリが生成されず、スキップ扱いになること
- 両ファイル存在時に 2 ファイルのコピーとマウントエントリが生成されること
- 既存テスト(cargo test 全件)を壊さないこと

### 4. ドキュメント

- README: codex レビュー対応の節を追加(必要条件: ホストで codex ログイン済み =
  `~/.codex/auth.json` が存在すること。持ち込むファイルは 2 つだけであること。
  codex の更新は `init --rebuild` であること)
- SECURITY.md: 注入する認証ファイルと持ち込まないものの一覧を追記

## 制約

- ブランチ: main から `feat/codex-in-container` を切って作業。コミット可・**push / PR 作成は禁止**
- `unwrap()` / `expect()` 本体コード禁止(テストは可)。全エラーパスに運用者向け情報
- `cargo fmt` / `cargo clippy --all-targets -- -D warnings` / `cargo test` を通すこと
- Dockerfile 変更の実機検証: 可能なら `docker build` で codex 導入レイヤーの成功を確認する。
  ネットワーク等で不可能な場合はその旨を報告(推測で「動くはず」と言わない)

## 完了条件(evidence)

1. `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` の実際の出力
2. Dockerfile の docker build 検証結果(codex --version の出力を含む)
3. `git log --oneline main..HEAD` と `git diff --stat`
4. 注入ロジックのテスト名一覧

## 差し戻し条件

- musl バイナリが Debian bookworm-slim で動作しない等、前提が崩れた場合
- copy-then-mount の rw マウントが既存のマウント構成と整合しない場合
- その他 spec にない非自明な判断が必要になった場合は、実装せず理由を最終出力で報告すること
