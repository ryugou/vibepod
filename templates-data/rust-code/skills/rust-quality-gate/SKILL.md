---
name: rust-quality-gate
description: Rust コードを commit する前に fmt / clippy / test を全通しする。use when コミットを作成する直前、または「完了」と宣言する直前
---

# Rust Quality Gate

commit / PR 提出 / 完了宣言の **直前に必ず実行する**。1 つでも失敗したら未完了として扱い、修正してから再実行する。

## Step 1: `cargo fmt`

```bash
cargo fmt
```

期待: 終了コード 0、差分があれば自動で整形されている。

確認:

```bash
cargo fmt --check
```

期待: 終了コード 0、出力なし（差分ゼロ）。

**CI で `cargo fmt --check` が走るので、ここを飛ばすと CI が落ちる**。

## Step 2: `cargo clippy`

```bash
cargo clippy --all-targets -- -D warnings
```

期待: 終了コード 0、warning ゼロ。

- `--all-targets` でテスト・example・bin も含めて lint する。
- `-D warnings` で警告をエラー扱いにする。`#[allow(...)]` で個別に抑制する場合は **理由コメントを必ず添える**:

  ```rust
  // clippy が誤検知するケース: このループは collect して後段の
  // ownership を譲渡する必要があるため iterator chain に単純化できない。
  #[allow(clippy::needless_collect)]
  let items: Vec<_> = iter.collect();
  process(items);
  ```

## Step 3: `cargo test`

```bash
cargo test
```

期待: 全 test pass、`test result: ok. N passed; 0 failed` が全 test binary で出る。

- 1 つでも FAIL があれば完了ではない。修正する。
- `ignored` テストは原則ゼロ（`#[ignore]` は明確な理由がある場合のみ、コメント必須）。
- 新規実装・修正をしたら、**対応するテストが追加 / 更新されているかを確認** する。未変更ならテストの欠落を疑う。

## Step 4: （推奨）`cargo check` / `cargo build`

上 3 つが通れば基本 OK だが、release ビルドで差が出ることがあるので気になる場合:

```bash
cargo check --all-targets
# または
cargo build --release
```

## 禁止事項

- 1 つでも失敗したまま「完了」と宣言する
- `cargo test` の出力を実際に確認せず「動くはず」と言う
- `--offline` / 一部ターゲット除外 / 一部テストスキップで実行を偽装する
- `#[ignore]` をコメント無しで足す
- `#[allow(...)]` を理由コメント無しで足す

## Self-check

- [ ] `cargo fmt --check` → OK
- [ ] `cargo clippy --all-targets -- -D warnings` → OK
- [ ] `cargo test` → 全 pass、ignored ゼロ（または理由コメント付き）
- [ ] 変更箇所に対応するテストが追加・更新されている
