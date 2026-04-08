---
name: rust-tdd-cycle
description: Rust コードを 1 機能ずつ TDD サイクル（失敗テスト → 実装 → 緑 → refactor → commit）で機械的に進める。use when 新しい関数 / 型 / モジュールを実装または修正するとき
---

# Rust TDD Cycle

Rust コードを書くときは、常に以下の 6 ステップを順に実行する。ステップを飛ばさない。

## Step 1: 失敗テストを書く

これから実装する挙動を表現するテストを 1 つ書く。

- `#[cfg(test)] mod tests` 内、または `tests/` ディレクトリに追加。
- テスト名は「何が期待されるか」を表す: `test_parse_returns_err_on_empty_input` 等。
- 1 テストで 1 つの振る舞い。複数の assertion を混ぜない（別テストに分ける）。
- 入力・期待出力を具体的に書く（「正しく動く」のような抽象テスト禁止）。

```rust
#[test]
fn test_parse_valid_iso8601_datetime() {
    let result = parse_datetime("2026-04-08T12:34:56Z");
    assert!(result.is_ok());
    let dt = result.unwrap();
    assert_eq!(dt.year(), 2026);
}
```

## Step 2: テストを実行して **赤** を確認

```bash
cargo test <test_name> -- --nocapture
```

期待: 失敗する。コンパイルエラー（未実装）でも OK、assertion エラーでも OK。
**実行せずに Step 3 に進まない**。赤の確認は「テストが本当に失敗しうるか」の担保。

## Step 3: 最小実装

テストが緑になる **最小の** 実装を書く。

- YAGNI: 今のテストが通るだけのコードを書く。「ついでに後で使いそうな機能」は書かない。
- panic しない (`unwrap()` / `expect()` 禁止、`?` を使う)。
- 既存スタイルに従う（周囲のコードを読んで命名・構造を合わせる）。

## Step 4: テストを実行して **緑** を確認

```bash
cargo test <test_name> -- --nocapture
```

期待: pass する。他のテストも壊していないか `cargo test` で全体確認する。

## Step 5: refactor（必要なら）

- 重複 / 命名 / 責務分割を見直す。
- ただし **テストが緑のまま** であることを常に確認する（refactor 中に壊れたら即 revert して再検討）。
- 3 回目の重複まで DRY しない（YAGNI）。

## Step 6: commit

ここまでの変更を 1 つの論理コミットにまとめる。

```bash
git add <changed files>
git commit -m "feat: parse ISO 8601 datetime"
```

Conventional Commits に準拠（`feat:` / `fix:` / `refactor:` / `test:` 等）。

## 禁止事項

- テストを書かずに実装を始める
- 赤の確認をスキップする
- 一度に複数の機能を実装する（1 サイクル = 1 機能）
- `unwrap()` / `expect()` でテストを緑にする（テストコード内は許容）
- テストを「緑になるように」書き換える（挙動変更が意図的ならテストと実装を同時に変える、ただし別コミット）
