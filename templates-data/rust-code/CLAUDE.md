# rust-code template

vibepod の公式 template。**Rust プロジェクトで品質の高いコードを autonomous に書く** ための環境を提供する。

## 役割

あなたはこの template が有効化された Claude Code セッションで、Rust コードの実装・修正・リファクタリングを行う。動くだけでなく、所有権設計・エラーハンドリング・テスト戦略までを含めて「仕事として通せる」品質を出す。

## 鉄則（例外なし）

1. **`unwrap()` / `expect()` の使用を原則禁止**。代わりに `?` で伝播、`anyhow::Context` で文脈付与、`match` / `if let` でハンドリング。
   - 例外: テストコード内。
   - 例外: regex の compile 等、パターンが literal で正しいことが論理的に自明な場合のみ。その場合は「なぜ panic しないか」を必ずコメントで明記する。

2. **commit 前に必ず実行**:
   - `cargo fmt` — フォーマット。CI で `cargo fmt --check` が走るので崩れていると落ちる。
   - `cargo clippy --all-targets -- -D warnings` — lint を警告ゼロまで。
   - `cargo test` — 全テスト pass。

3. **TDD 順守**: 失敗テスト → 実行して赤を確認 → 最小実装 → 実行して緑を確認 → 必要なら refactor → commit。「テスト無しで先に実装」は禁止。

4. **premature abstraction を嫌う**: 3 回目の重複まで DRY せず、まず具象を書く。trait / generic は今必要な理由を説明できない限り導入しない。

5. **`pub` 最小化**: crate 内だけで使うものは `pub(crate)`、module 内だけなら `pub(super)` / 非 pub。公開 API は明示的な設計判断の対象にする。

6. **public item には doc コメント**: `///` で用途・引数・戻り値・panic する条件（あれば）を書く。crate 内 item でも非自明なら書く。

## ワークフロー

新しい機能追加・バグ修正・リファクタリングは以下の順で進める:

1. **要件確認**: 何を達成したいかを自分の言葉で言い直し、制約を洗い出す。不明点はユーザーに聞く。
2. **設計スケッチ**: データ型 / 関数シグネチャ / エラー型を先にスケッチする。所有権境界（`&T` / `&mut T` / `T` / `Arc<T>` 等）を意識する。
3. **テスト駆動実装**: `rust-tdd-cycle` skill に従う。
4. **quality gate**: `rust-quality-gate` skill に従い、fmt / clippy / test を全通しする。
5. **self review**: 変更の最小化、コメントの過不足、命名、dead code / unused import が無いか確認。
6. **commit**: Conventional Commits に準拠（`feat:` / `fix:` / `refactor:` / `test:` / `docs:` / `chore:` 等）。commit は論理単位で小さく。

## エラーハンドリング

- アプリケーションコードは `anyhow::Result<T>`、library コードは独自 error type (`thiserror` 推奨) を使う。
- エラーに必ず文脈を付ける: `op().with_context(|| format!("while doing X with {}", arg))?`。
- panic が本当に必要な場合（内部不変条件の破壊）はコメントで理由を明記する。プロダクションパスでの `unwrap()` / `expect()` は原則禁止。

## テスト設計

- **振る舞いをテストする**、内部実装詳細をテストしない。private な helper の細部ではなく、public API の入出力を assert する。
- **edge case を網羅**: 空入力、境界値、overflow、エラーパス、並行性（該当するなら）。
- **テスト名は「何が期待されるか」を表す**: `test_parse_returns_err_on_empty_input` のように。

## コード review 観点（self-review で必ず確認）

- 所有権・lifetime は最小・自然か（不要な `clone()` / `Arc` が無いか）
- エラーハンドリングは網羅・一貫しているか（`unwrap()` を漏らしていないか）
- 関数が大きすぎないか（目安 ~50 行、越えたら分解検討）
- 命名は意図を表すか（短縮・略語に頼っていないか）
- テストが実装に追随しているか（変更箇所にテストが無いなら追加）

## skill / agent

この template には以下が含まれる:

- `skills/rust-tdd-cycle` — TDD の 1 サイクルを機械的に実行させる
- `skills/rust-error-discipline` — エラーハンドリング規律
- `skills/rust-quality-gate` — commit 前チェックの自動実行
- `agents/rust-implementer.md` — 実装担当（senior Rust engineer 人格）
- `agents/rust-test-designer.md` — テスト設計担当

必要に応じてこれらを起動して作業する。

## plugin について

この template は **plugin を bundle せず、plugin に依存しない**。
template mode では vibepod がマウントするのは
`CLAUDE.md` / `skills/` / `agents/` / `settings.json` のみで、
host 側の `~/.claude/plugins/` はマウントされない。したがって
template mode 実行中に `superpowers` / `rust-analyzer-lsp` などの
plugin は **使えない** と考える。

必要な振る舞いはすべて上記の skill / agent に直接記述済み
（TDD / エラー規律 / quality gate / 所有権設計 / テスト設計）。
plugin 相当の高度な機能（LSP ベースの型情報など）が必要な作業は、
interactive mode (`vibepod run` without `--template`) で host の
`~/.claude` 環境を使うことを推奨する。

Phase 5 以降で template から plugin を宣言的に配布・同期する
mechanism を検討するまでは、この template は
「plugin なしでも品質の高い Rust コードを書くための skill/agent 集合」
として閉じた状態で使う。
