# Swift プロファイルとセッション堅牢化 設計

| 項目 | 内容 |
| ---- | ---- |
| 目的 | (1) 言語プロファイル `swift` によるイメージバリアント、(2) タイムアウト時の workspace 保全、(3) `--prompt-file` オプションの仕様を決める |
| 読者 | 実装エージェント（kaneko）、レビュアー |
| 正本の範囲 | 上記 3 機能の設定スキーマ、イメージ名規則、Dockerfile 構成、タイムアウト後処理、CLI オプション仕様、テスト・受入条件 |
| 関連文書 | `docs/design.md`（全体アーキテクチャ）、`templates/Dockerfile`（実装後はバージョン・SHA256 の正本） |

## 1. スコープ

- 要件1: `.vibepod/config.toml` の `profile = "swift"` 指定時のみ、Swift toolchain + SwiftLint を含むイメージバリアントを使う。非 Swift プロジェクトのイメージ・起動時間へ影響を与えない。
- 要件2: `--prompt` セッションのタイムアウト打ち切り時に workspace を git reset しない（全プロジェクト共通）。
- 要件3: `--prompt-file <path>` を追加し、ホストシェルの解釈を経由せずプロンプトを渡せるようにする（全プロジェクト共通）。

バージョン bump・リリースは本設計の対象外（別 PR で行う）。

## 2. 要件1: Swift プロファイル

### 2.1 設定スキーマ

`RunConfig`（`src/config/vibepod_config.rs`）へ `profile: Option<String>` を追加する。

- 記述場所: `.vibepod/config.toml`（プロジェクト）と `~/.config/vibepod/config.toml`（グローバル）の `[run]`。マージは `lang` と同一規則（プロジェクト優先のフィールド単位マージ）。
- CLI フラグは追加しない。設定ファイル専用とする。
- 有効値は `"swift"` のみ。他の値は `vibepod run` 開始時にエラーとし、メッセージに有効値一覧を含める。

```toml
[run]
profile = "swift"
```

### 2.2 イメージ名規則

profile 指定時のイメージ名は、グローバル設定 `image` の名前部へ `-<profile>` を挿入して導出する（タグは維持）。

- `vibepod-claude:latest` + `profile = "swift"` → `vibepod-claude-swift:latest`
- タグなし（`vibepod-claude`）→ `vibepod-claude-swift`

導出は純関数 `image_for_profile(base_image: &str, profile: &str) -> String` として実装し、ユニットテストする。profile 未指定時は現行どおり `image` をそのまま使う。

### 2.3 Dockerfile 構成

`templates/Dockerfile` を単一ファイルのまま multi-stage 化する。BuildKit（Docker 23+）を前提とし、未使用ステージはビルドされない。

```dockerfile
ARG VIBEPOD_PROFILE=default

# distro エイリアス（2.3.1 参照）
FROM debian:bookworm-slim AS distro-default
FROM debian:trixie-slim AS distro-swift

FROM distro-${VIBEPOD_PROFILE} AS base
# （現行の全内容: apt, gh, vibepod ユーザー, claude, codex, USER vibepod, WORKDIR, CMD）

FROM base AS profile-default

FROM base AS profile-swift
USER root
# Swift レイヤー（2.4）
USER vibepod

FROM profile-${VIBEPOD_PROFILE}
```

- `base` ステージの命令列自体は現行 Dockerfile と同一に保ち、default イメージのレイヤー内容・サイズを変えない（ベースディストリのみ 2.3.1 の理由で profile ごとに切り替える）。
- `build_image_for()`（`src/cli/init.rs`）へ profile 引数を追加し、build args に `VIBEPOD_PROFILE` を渡す。

#### 2.3.1 ベースディストリの決定（bookworm / trixie）

実装当初は `base` を一律 `debian:bookworm-slim` とする設計だったが、実ビルド検証で Critical 欠陥が判明したため、profile ごとに distro を切り替える方式に変更した。

- **事実**: SwiftLint 0.65.0 公式 Linux バイナリ（`swiftlint_linux_*.zip`）は `GLIBC_2.38` / `GLIBCXX_3.4.32` を要求する。`debian:bookworm-slim` は glibc 2.36・GCC12 系 libstdc++（`GLIBCXX` ≤ 3.4.30）までしか持たず、Dockerfile 末尾のスモークチェック `RUN swift --version && swiftlint version` が必ずリンクエラーで失敗する（実測済み）。
- **決定**: `profile = "swift"` のときのみベースを `debian:trixie-slim`（glibc 2.41）に変更する。`profile` 未指定（default）は `debian:bookworm-slim` のまま変更しない — 非 Swift プロジェクトのイメージ・起動時間に影響を与えない要件（1章 要件1）を満たすため。
- **Swift toolchain tarball**: swift.org は本設計時点（2026-08）で Debian 13(trixie) 向けネイティブ tarball を配布していない（`swift-${SWIFT_VERSION}-RELEASE-debian13*.tar.gz` は 404）。そのため 2.4 の Swift toolchain は引き続き debian12 向け tarball を使う。glibc は後方互換があるため trixie 上でも動作するが、この「動作する」は以下の範囲でのみ実測確認済みであり、それ以上を保証するものではない: `swift --version` 成功、Foundation-only フィクスチャの `swift test` 完走、`swiftlint lint --strict` のホスト同一判定。
- **既知の制約（未検証範囲）**: debian12 版 Swift toolchain の `lldb` は `libpython3.11.so.1.0` に SONAME 固定でリンクしており、trixie は python3.13 のみで `libpython3.11` パッケージ自体が存在しないため、コンテナ内で `lldb` / `swift repl` は dynamic linker エラーで起動しない。`swift build` / `swift test` / `swiftc` / `swiftlint` には影響しない（README の Constraints にも記載）。
- **教訓（次回 SWIFT_VERSION / distro bump 時の判断材料）**: 「glibc 後方互換だから動く」は可搬性の必要条件であって十分条件ではない。`lldb` のように distro 固有パッケージ（`libpython3.<minor>`）への versioned SONAME 依存を持つバイナリは、glibc が後方互換でも別の壁で壊れる。distro を跨いだ動作確認は「起動する」だけでなく「pin しているバイナリが依存する共有ライブラリの SONAME が対象 distro に存在するか」まで機能単位で確認すること。
- **副作用**: trixie には `libstdc++-12-dev` パッケージが存在しない（gcc-12 系リポジトリが無い）ため、2.4 の実行時依存パッケージリストは `libstdc++-14-dev` に読み替える。apt install の成功を実測済み。
- **実装**: `ARG VIBEPOD_PROFILE` の値（`default` / `swift`）をそのまま distro エイリアスのステージ名サフィックスに使う（`FROM distro-${VIBEPOD_PROFILE} AS base`）。

### 2.4 Swift レイヤーの内容

すべて codex CLI と同じ「バージョン pin + アーキ別 SHA256 テーブル + ダウンロード後検証」方式とする。`latest` エスケープハッチは設けない。更新はバージョン ARG の bump と SHA256 テーブルへの追記の後、`vibepod init --rebuild`（手順を README に記載）。

1. 実行時依存パッケージ（公式 swiftlang/swift-docker `6.3/debian/12` と同一 + zip 展開用 unzip。
   ただし trixie には `libstdc++-12-dev` が存在しないため `libstdc++-14-dev` に読み替え — 2.3.1 参照）:
   `binutils libicu-dev libcurl4-openssl-dev libedit-dev libsqlite3-dev libncurses-dev libpython3-dev libxml2-dev pkg-config uuid-dev tzdata git gcc libstdc++-14-dev unzip`
2. Swift toolchain（`ARG SWIFT_VERSION=6.3.3`。ホスト Xcode 26.6 / Swift 6.3.3 とバージョン一致）:
   - URL: `https://download.swift.org/swift-${SWIFT_VERSION}-release/debian12${suffix}/swift-${SWIFT_VERSION}-RELEASE/swift-${SWIFT_VERSION}-RELEASE-debian12${suffix}.tar.gz`
     - `suffix`: amd64 → 空、arm64 → `-aarch64`
   - SHA256（6.3.3）:
     - debian12-aarch64: `ecba8ef87b54a5048d466af500f3169c939a6b8a2cb7c600f76b5184457f293a`
     - debian12 (x86_64): `19e0c78cad5418ad48bfa87aa20c53ac9ac9996d1695d04dd94f7c7ea4eb133f`
   - `/opt/swift` へ展開し、`ENV PATH=/opt/swift/usr/bin:$PATH` で `swift` を通す。
3. SwiftLint（`ARG SWIFTLINT_VERSION=0.65.0`）:
   - URL: `https://github.com/realm/SwiftLint/releases/download/${SWIFTLINT_VERSION}/swiftlint_linux_<arch>.zip`（arch: `arm64` / `amd64`）
   - SHA256（0.65.0）:
     - linux_arm64: `12d3b84bc5b69ae13a99a5a5c79904f9ce25867f099f6368d0037854f9ee6c26`
     - linux_amd64: `79306a34e5c7cc55a220cd108cbb861dcad5f10138dcdf261e2624ae8b0a486b`
   - `/usr/local/bin/swiftlint` へ配置する。
4. 検証: `RUN swift --version && swiftlint version`

実装後は `templates/Dockerfile` のテーブルをバージョン・SHA256 の正本とする。

### 2.5 実行フローへの組み込み（`src/cli/run/prepare.rs`）

1. `VibepodConfig` から profile を読み、検証する（2.1）。
2. イメージ名を `image_for_profile` で決定し、`ensure_image_available` へ渡す（swift イメージ未存在時は既存の自動ビルド経路で profile 付きビルド）。
3. コンテナラベル比較（step 9b）へ profile を追加する。既存コンテナと profile が異なる場合の挙動は、既存の他項目（lang / env ハッシュ）の変更検知と同一とする。
4. profile 未指定かつ workspace 直下に `Package.swift` が存在する場合、`.vibepod/config.toml` に `profile = "swift"` を設定するよう促す 1 行の注意を stderr に出す（実行は継続する）。

`vibepod init` は現行どおり default イメージのみビルドする。`vibepod init --rebuild` は default を再ビルドし、加えて swift バリアントイメージ（`image_for_profile` で導出した名前）が存在する場合はそれも同じ引数で再ビルドする。

### 2.6 キャッシュとネットワーク

- SwiftPM キャッシュ（`~/.swiftpm`、`~/.cache/org.swift.swiftpm`、モジュールキャッシュ）はコンテナの home 配下に置かれ、非 disposable コンテナは停止保持で再利用されるため、`vibepod run` をまたいで保持される。追加実装は不要。`--worktree`（disposable）はキャッシュを保持しない（cargo 等の既存言語と同じ）。
- workspace に残る生成物は SwiftPM 既定の `.build/` のみ。
- パッケージ解決には外向き HTTPS が必要。既定のコンテナはネットワーク許可のため追加実装は不要だが、`--no-network` と profile = "swift" の併用ではパッケージ解決が失敗することを README に記載する。

### 2.7 ドキュメント（README）

以下を README に記載する。

- `profile = "swift"` の設定方法と、swift イメージが初回 run 時に自動ビルドされること。
- 導入バージョン（Swift 6.3.3 / SwiftLint 0.65.0）と更新手順（ARG bump + SHA256 テーブル追記 + `vibepod init --rebuild`）。SwiftLint はホストとバージョン差があると判定差が出るため、ホスト側と揃えることを明記する。
- 制約: Linux Swift で動くのは Foundation-only / 純 SwiftPM パッケージのみ。Apple フレームワーク（UIKit / Vision / Core Image / StoreKit 等）・xcodebuild・シミュレータは対象外。Linux の corelibs-foundation は Darwin と細部が異なるため、コンテナで green でも macOS（ホスト / CI）検証を代替しない。

### 2.8 受入条件

- コンテナ内で `swift --version` が成功し、6.3.3 を表示する。
- `swift package init --type library` で生成した Foundation-only フィクスチャパッケージの `swift test` がコンテナ内で完走する。
- コンテナ内 `swiftlint version` が 0.65.0 を表示し、フィクスチャで `swiftlint lint --strict` が実行できる。
- profile 未指定のプロジェクトでは、使用イメージ・起動シーケンスが現行と変わらない。

## 3. 要件2: タイムアウト時の workspace 保全

### 3.1 採用する設計

`src/cli/run/prompt.rs` のタイムアウト後処理（現行 410〜437 行）から git 操作（`reset --mixed` / `reset --hard` / `clean -fd`）と `mark_restored` 呼び出しをすべて削除する。タイムアウト時は workspace に一切触れない。

- エージェントのコミット・未コミット変更・未追跡ファイルはそのまま残す（stash 退避はしない）。
- コンテナ内 claude の kill、コンテナ停止／削除、非ゼロ終了（`anyhow::bail!`）は現行どおり維持する。
- セッションは restored 扱いにしないため、`vibepod restore` による手動復元の対象として残る。

### 3.2 終了メッセージ

タイムアウト時の stderr メッセージを次の内容に含める。

- 中断理由（実時間上限 / ストリーム無出力）と上限値、ログパス（現行どおり）。
- エージェントの変更（コミット・未コミットとも）が workspace に残っている旨（変更が
  無ければその旨）。
- 復元手順は「未コミット変更が残っている」「コミット済みのみで clean」「開始時点から
  無変更」の3状態と、`--worktree` 実行かどうかで出し分ける。`vibepod restore` は
  未コミット変更が残っていると使えない（`restore.rs` が bail する）ため、
  `--worktree` 実行時は `vibepod restore` を案内せず `.worktrees/<dir>` 側での
  確認・破棄コマンドに置き換える。

**改訂履歴**: フル再レビュー F2/F3（状態別の出し分けを導入）→ MJ1/mn1（破棄
コマンドを `git reset --hard && git clean -fd` へ修正、worktree 削除案内に
`--force` 分岐を追加）→ Q3（simplify 指摘: cwd/worktree の3状態テンプレート
二重実装を `git(args)` クロージャで1本化）を経て現在の形になった。**分岐の詳細・
各コマンドを選んだ判断根拠の正本は実装（`render_timeout_message`、
`src/cli/run/mod.rs`）の doc コメント。** このファイルには逐語コピーを置かず、
実装ドキュメントと2箇所で食い違う余地を作らない。関数自体は引き続き git
コマンドを一切呼ばない純関数のまま維持する（呼び出し元の `prompt.rs` が
read-only な `git::get_head_hash` / `git::has_uncommitted_changes` で調べた
`head_advanced: bool` / `has_uncommitted: bool` / `worktree_dir: Option<&str>`
を渡す）。

### 3.3 横断更新

- reset 挙動を検証する既存テストを新仕様（タイムアウト時に git 操作しない）へ置き換える。
- リポジトリ内ドキュメント（README、`docs/design.md` 等）の「タイムアウト時にリセットする」旨の記述を全文検索し、保全仕様へ置き換える。`src/cli/mod.rs` の `--timeout` ヘルプ文も「workspace の変更は保持される」を反映する。

### 3.4 受入条件

- タイムアウトしたセッションの後、エージェントが作成した未コミットファイル・変更・コミットが workspace に残り参照できる。
- タイムアウト終了は引き続き非ゼロ終了する。

## 4. 要件3: `--prompt-file`

### 4.1 採用する設計

`vibepod run` に `--prompt-file <path>` を追加する。

- clap 定義: `prompt_file: Option<String>`、`conflicts_with = "prompt"`（同時指定は clap がエラーにする）。
- 読み込みは run 実行の冒頭（prepare より前）で行い、`std::fs::read_to_string` の内容を**無加工で**（trim せず）内部の prompt 値へ格納する。以降の全経路（ロック、タイムアウト、`claude -p` への受け渡し、ログ、サマリ）は `--prompt` と完全に同一。
- エラー: ファイルが読めない場合はパス付きで失敗させる。内容が空または空白のみの場合もエラーとする。
- プロンプト内容は既存経路どおり process 引数として `docker exec … bash --login -c 'exec claude "$@"' -- -p <内容>` に渡るため、コンテナ側でもシェル展開されない。

### 4.2 受入条件

- 山括弧・波括弧・バッククォート・`$` を含むプロンプトファイルで `vibepod run --prompt-file` が起動し、内容が無加工で `claude -p` の引数に渡る。
- `--prompt` と `--prompt-file` の同時指定はエラーになる。
- README のオプション一覧に `--prompt-file` を追記する。

## 5. テスト計画

ユニットテスト（既存スタイル: 純関数 + tempfile）:

1. `RunConfig.profile` のパースとマージ（プロジェクト優先、グローバルのみ、未指定）。
2. profile 検証: `"swift"` は許可、それ以外はエラー。
3. `image_for_profile`: タグあり／タグなし。
4. `build_image_for` 相当のビルド引数組み立てに `VIBEPOD_PROFILE` が含まれる。
5. `--prompt-file`: 特殊文字（`< > { } \` $`）を含むファイル内容が無加工で prompt 値になる。存在しないファイル・空ファイルはエラー。clap の conflict。
6. タイムアウト後処理: git 操作を行わないことを検証する形へ既存テストを更新（reset を期待する旧テストは削除）。

E2E（ホストで実施。ユニットテストの対象外）:

1. swift イメージのビルドと、コンテナ内 `swift --version` / `swiftlint version`。
2. フィクスチャパッケージの `swift test` と `swiftlint lint --strict`。
3. 検証用リポジトリで `--prompt-file`（特殊文字入り）実行。
4. 検証用リポジトリで短い `--timeout` を発生させ、変更残存を確認。

## 6. 実装順

1. Run A: 要件2 + 要件3（Rust のみ、互いに独立だが同一ファイルを触るため一括）。
2. Run B: 要件1（config / prepare / init / Dockerfile / README）。

Run A と Run B は `src/cli/mod.rs`・`src/cli/run/prepare.rs` が重なるため直列に実施する。
