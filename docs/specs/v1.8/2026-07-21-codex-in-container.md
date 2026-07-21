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

## codex レビュー指摘対応(round 1、2026-07-21)

初回実装に対する codex レビューで以下2件の指摘を受けた。いずれも実在を確認済みで、対応必須。

### P1: ホストで auth.json を削除してもステージ済み認証が残る(`prepare_codex_mount` の early return)

`~/.codex/auth.json` が無い場合に警告して `Ok(None)` を返すだけでは、
**過去の run でステージ済みの** `<runtime>/codex/auth.json` が残置され、既存コンテナの
bind mount 経由で使われ続ける。「codex review is unavailable」という警告と実態が矛盾する。

対応: `!has_auth` の場合、return 前にステージディレクトリの**中身**(auth.json / config.toml)を
削除すること。ディレクトリ自体は削除しない(実行中コンテナの bind mount の inode を壊さないため。
中身が空なら codex は未認証となり、取り消しの意味論が正しく成立する)。削除失敗はエラーを
握りつぶさず context 付きで伝播する。

### P2: ホストで config.toml を削除してもステージ済みの旧設定が残る(コピーループ)

auth.json は存在し config.toml だけ削除されたケースで、現在の entries のコピーのみ行い
宛先の掃除をしないため、削除済み設定が無期限に使われ続ける。

対応: コピー前に宛先を allowlist と突き合わせて**リコンサイル**する。
`HOST_CODEX_ALLOWLIST` にあるが今回の entries に無いファイルは宛先から削除してからコピーする。

### テスト追加(round 1 対応分)

- 事前にステージ済み auth.json がある状態で host の auth.json を消して呼ぶ → ステージも消え `None`
- 事前に両ファイルステージ済みで host の config.toml だけ消して呼ぶ → ステージの config.toml が消え auth.json は更新される
- ステージディレクトリ自体は上記どちらのケースでも削除されないこと

## codex レビュー指摘対応(round 2、2026-07-21)

round 1 対応(P1/P2)は解消を確認。新規指摘1件、対応必須。

### P1: codex マウントの有無が構成差分として検出されず、既存コンテナで codex が黙って使えない(`src/cli/run/mod.rs:596-599` 付近)

bind mount はコンテナ作成時に固定されるため、(a) 本機能より前に作られたコンテナ、
(b) 作成時に `~/.codex/auth.json` が無く後からログインしたケースでは、
`codex_dir` を用意しても `/home/vibepod/.codex` はマウントされない。
現在のマウント比較ラベル(`build_config_labels` の mounts)に codex の有無が含まれないため
警告も出ず、ユーザーは `--new` するまでコンテナ内 codex レビューが使えない理由に気づけない。

対応: 既存の sanitized_settings と同様の方式で、codex マウントの有無を mounts ラベルの
構成要素に含めること(例: `codex=/home/vibepod/.codex` のような専用 prefix エントリ)。
これにより既存の「mount set 変更検知 → 警告と `--new` 案内」のゲートが codex の
追加・削除を自然に検出する。後方互換(既存コンテナのラベルとの比較で不当に
常時警告にならないこと)に注意し、v1.4.3 の legacy 正規化(`normalize_mounts_label_legacy`)の
前例に倣うこと。

### テスト追加(round 2 対応分)

- codex マウントあり/なしそれぞれでラベルが安定して生成されること
- codex の有無が変わった場合に構成差分として検出されること(警告経路)
- codex の有無が同じ場合は差分として検出されないこと

## codex レビュー指摘対応(round 3、2026-07-21)

round 2 対応は解消を確認。新規指摘1件、対応必須。

### P1: コンテナ内 codex が更新した auth.json を、次回 run が古いホストコピーで無条件上書きする(`src/cli/run/mod.rs:576-579` 付近)

ステージは rw マウントであり、コンテナ内 codex はトークンリフレッシュ時にステージ済み
`auth.json` を書き換える。次回 `vibepod run` のコピー処理はこれをホスト側の(古い)
`auth.json` で無条件に上書きするため、リフレッシュトークンがローテーションされる場合、
以後コンテナ内 codex が認証不能になり得る。

対応方針(確定。ホストへの書き戻しは「ホスト原本に触れない」原則に反するため採用しない):
- **auth.json のみ「新しい方を保持」**: コピー前に、ステージ済み `auth.json` が存在し
  かつホスト側より mtime が新しい場合は上書きせず保持する。ホストの方が新しい
  (=再ログイン等)場合は従来どおりホスト側で上書きする
- `config.toml` は認証ではなく設定のため従来どおり常にホスト優先で上書き
- round 1 の意味論は維持: ホストで `auth.json` が削除されていれば(明示的な取り消し)、
  ステージの新旧に関わらず中身を削除して `None`
- mtime 比較はテスト可能な形(比較ロジックの純関数化、時刻はファイルシステム経由で
  制御可能なテストフィクスチャ)で実装すること

### テスト追加(round 3 対応分)

- ステージの auth.json がホストより新しい → 保持され、上書きされない
- ホストの auth.json がステージより新しい → ホスト側で上書きされる
- ステージの config.toml がホストより新しくても → 常にホスト側で上書きされる
- ホストの auth.json 削除 → ステージの新旧に関わらず中身削除+None(round 1 の回帰確認)

## codex レビュー指摘対応(round 4、2026-07-21)

round 3 対応は解消を確認。新規指摘1件、対応必須。

### P1: disposable 実行の終了処理が、更新済み codex 認証ごと runtime dir を削除する(`src/cli/run/mod.rs:553-557` 付近)

`--new` / worktree 等の disposable 実行では終了時に `ctx.runtime_dir` が削除されるため、
コンテナ内 codex がリフレッシュした auth.json(ローテーション時は唯一の有効コピー)が失われる。

対応方針(確定): ステージ位置を per-container からユーザー単位の共有ステージへ移す。
- ステージ先を `<runtime>/<container>/codex/` から **`<config_dir>/codex/`**(全コンテナ共有、
  dir 0700 / file 0600)に変更する。per-container cleanup の削除対象外になるため、
  退避・サルベージ機構は不要になる(構造的解決)
- round 1〜3 の意味論は共有ステージにそのまま適用:
  ホスト削除→wipe+None / allowlist リコンサイル / auth.json は keep-newer / config.toml はホスト優先
- round 2 のマウントラベル(codex marker)はマウント先が固定パスのため presence 判定のみで維持
- 併走コンテナが同一 auth.json を共有することになる点は、コード内コメントと SECURITY.md に
  明記する(codex の書き込みはファイル置換であり実害は限定的。per-container コピー方式でも
  provider 側ローテーション問題は同様に存在するため、集約の方が総合的に安全)
- 既存テストのパスをこの構成に追随させ、「disposable cleanup 後も共有ステージが残る」ことを
  検証するテストを追加すること

## codex レビュー指摘対応(round 5、2026-07-21)

round 4 対応は解消を確認。新規指摘2件(いずれもセキュリティ境界)、対応必須。

### P1-a: コピー先 symlink 経由のホストファイル上書きを防ぐ(`src/cli/run/mod.rs:632-633` 付近)

コンテナ内プロセスが rw マウントされたステージ内の `auth.json` / `config.toml` を
ホスト任意パスへの symlink に差し替えた場合、次回 run の `std::fs::copy` がリンク先を
辿ってそのホストファイルを上書きする(コンテナ→ホストの書き込み境界の破れ)。
※この欠陥は round 4 以前の per-container ステージにも存在した。

対応:
- コピー・削除・mtime 判定などステージ内ファイルを扱うすべての箇所で、対象が
  symlink(`symlink_metadata` で判定)なら**辿らずに削除して警告を出す**
  (「staged codex file was a symlink; removed (possible container tampering)」等、
  運用者が改ざんの可能性に気づける文言)
- ホスト→ステージのコピーは「ステージ内の一時ファイルに書いてから rename で
  アトミックに置換」する方式に変更し、置換直前にも宛先の symlink 検査を行う

### P1-b: ステージへの allowlist 外ファイルの永続化を防ぐ(`src/runtime/docker.rs:68-69` 付近)

`.codex` ディレクトリ全体が rw のため、コンテナは `history.jsonl` やキャッシュ等
allowlist 外のファイルを共有ステージに作成でき、別リポジトリの全コンテナから
参照可能な形で永続化する。

対応:
- `prepare_codex_mount` のリコンサイルを「allowlist の欠落分の削除」から
  「**ステージ内に存在する allowlist 外のエントリ(ファイル・ディレクトリ・symlink)を
  すべて削除**」する完全リコンサイルに強化する。削除時は何を消したか stderr に出す
  (機微データの無言蓄積を防ぐ運用可視性のため。ファイル名のみ、内容は出さない)
- これにより allowlist 外の状態は「次回 run 開始時までに必ず掃除される」ことを保証する。
  マウント自体の書き込み制限(ro 化・ファイル単位マウント)は codex のトークン更新を
  壊すため行わない(spec の設計判断として明記)

### テスト追加(round 5 対応分)

- ステージの auth.json が symlink に差し替えられている → 辿らず削除され、ホストから再コピーされる
- ステージに allowlist 外のファイル・ディレクトリ(例: history.jsonl, cache/)がある → 次回 run 準備で削除される
- 完全リコンサイル後も allowlist 内の正当なファイルは維持される(round 1〜4 の回帰確認)

## codex レビュー指摘対応(round 6、2026-07-22)

round 5 対応の temp-file 機構自体への指摘2件。対応必須。

### P1: 固定名の一時ファイル(`<name>.tmp`)自体が symlink 差し替え可能(`src/cli/run/mod.rs:592-595`)

コンテナが `auth.json.tmp` をホスト任意パスへの symlink にしておくと、
次回 run の `std::fs::copy(src, &tmp)` がリンク先を辿り、dst への symlink 防御を迂回して
ホストファイルを上書きできる。

### P2: 並行 run が固定名 tmp を取り合い、片方が不定期に失敗する(同箇所)

複数 `vibepod run` の同時準備で、一方の rename 後に他方の chmod/rename が NotFound になる。

### 対応(両指摘を一挙に解決)

- 既存依存の `tempfile` クレートの `NamedTempFile::new_in(<ステージdir>)` を使う:
  予測不能な一意名 + `O_EXCL` 相当の排他生成であり、symlink 差し替えの標的にできず
  (P1 解消)、並行実行間で名前が衝突しない(P2 解消)
- 書き込み→パーミッション 0600 設定→`persist()`(rename)の順で置換する。
  `persist` の上書き先(dst)の symlink 検査は round 5 の防御をそのまま維持
- 手書きの `<name>.tmp` 方式のコードとテストは撤去し、置き換えたことをテストで固定する:
  - ステージ内に敵対的な `auth.json.tmp`(symlink)が残置されていても、コピーがそれを
    使用せず、ホストファイルが書き換えられないこと
  - (可能なら)並行呼び出しの簡易テスト、難しければ一意名生成の性質をもってテストに代える

### テスト追加(round 6 対応分)

- `copy_codex_asset_atomically_ignores_hostile_fixed_name_tmp_symlink`(P1): `reconcile_codex_stage_dir`
  を経由せず `copy_codex_asset_atomically` を直接呼び出し、ステージ内に敵対的な固定名
  `auth.json.tmp` symlink が残置されていても、コピー機構自体がそれを辿らず、
  symlink の指すホスト側ファイルが書き換えられないことを検証する(reconcile 経由の
  統合テストでは reconcile が copy より先に敵対的ファイルを掃除してしまい copy 機構自体の
  リグレッションを検知できない偽陽性になるため、直接呼び出しで検証する)
- `copy_codex_asset_atomically_survives_concurrent_calls_to_same_destination`(P2): 同一 `dst`
  への2スレッド並行呼び出しがいずれも成功し、`dst` が両者いずれかの完全な内容になる
  (競合エラー・部分書き込みが起きない)ことを検証する
- `tests/host_codex_assets.rs` の既存テストは
  `prepare_codex_mount_reconcile_sweeps_stale_fixed_name_tmp_symlink_before_copy` に改名し、
  round 5 の完全リコンサイルが copy 実行前に stale な固定名 tmp symlink を掃除することの
  回帰確認(round 6 の copy 機構自体の検証ではない)であることをコメントで明記した

## 差し戻し条件

- musl バイナリが Debian bookworm-slim で動作しない等、前提が崩れた場合
- copy-then-mount の rw マウントが既存のマウント構成と整合しない場合
- その他 spec にない非自明な判断が必要になった場合は、実装せず理由を最終出力で報告すること
