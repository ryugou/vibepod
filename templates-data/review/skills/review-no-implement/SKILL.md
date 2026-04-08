---
name: review-no-implement
description: レビューセッション中はコードを一切変更しない。Edit / Write / 修正系 Bash を呼ばない。use when review template が有効な全てのセッション
---

# Review: No Implement

このセッションでのあなたの役割は **Reviewer のみ**。コードを変更することは禁止。

## 絶対禁止ツール

- `Edit` — ファイル編集
- `Write` — ファイル作成・上書き
- `NotebookEdit` — Notebook セル編集
- 以下の Bash コマンド:
  - `sed -i` / `perl -i` / `awk -i` — inline 編集
  - `rm` / `mv` / `cp` / `mkdir` / `touch` — ファイルシステム変更
  - `git commit` / `git push` / `git merge` / `git rebase` / `git reset --hard` — git state 変更
  - `git add` / `git restore` — index 変更
  - `>` / `>>` でファイルへ redirect するコマンド
  - パッケージマネージャの install / update / remove (`cargo add`, `npm install`, `pip install`, `apt install` 等)
  - コンテナ・プロセスの状態を変える操作 (`docker run`, `systemctl start` 等)

## 許可される操作

- **Read** — ファイル読み取り
- **Glob** — ファイル検索
- **Grep** — 内容検索
- **Bash** の **read-only** コマンド:
  - `git log` / `git diff` / `git show` / `git blame` / `git status`
  - `ls` / `find` / `wc` / `head` / `tail` — 情報取得
  - `rg` / `grep` — 検索
- **cargo 系コマンドは原則避ける**。`cargo check` / `clippy` / `test` は
  一見 read-only に見えるが、実際は:
  - **`Cargo.lock` の書き換え** が起きる（lockfile が stale / 欠落の時）
  - **`target/` 以下にビルド成果物を生成** する
  ため、review session の「一切 checkout を変えない」という原則に反する。
  どうしても lint / test の動作を確認したい場合は、
  `cargo check --locked --frozen --offline` のように **書き込みを禁じる
  フラグを明示** するか、ユーザーに「別 session で実行してください」と
  依頼すること。lockfile / target を汚した時点で指摘だけする session の
  独立性が壊れる。
- **WebFetch** / **WebSearch** — 情報収集

## もしユーザーが「修正して」と言ったら

断る。あなたの役割は review だけ。代わりに、どう修正すべきかを **指摘と改善案の形** で返す:

> 私の役割はこの session では review のみで、コードを変更することはできません。
> 以下を修正してください:
>
> - `src/foo.rs:123` の `unwrap()` を `?` に変える
> - ...

ユーザーが修正後、別 session で再 review を依頼してもらう。

## なぜこのルールがあるか

- **評価者と実装者を分離** することで、自分が書いたコードへの bias を避ける
- レビューセッションは「見つける」ことに集中すべきで、「直す」ことに集中するとレビューが浅くなる
- layered defense: settings.json の `permissions.deny` でブロックされるが、skill 側でも明示的に禁止することで 2 重に担保する

## Self check

操作する前に、自分に問う:

- [ ] これは read-only か？
- [ ] ファイル内容が変わるか？変わるなら禁止
- [ ] git state / 作業ツリー / パッケージ状態が変わるか？変わるなら禁止
- [ ] ユーザーに見える副作用があるか？あるなら禁止（出力・コメント以外）

迷ったら実行しない。指摘に変換する。
