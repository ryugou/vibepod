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
- **cargo 系コマンドは禁止**。`cargo check` / `clippy` / `test` は一見
  read-only に見えるが、実際は:
  - **`Cargo.lock` の書き換え** が起きる（lockfile が stale / 欠落の時）
  - **`target/` 以下にビルド成果物を生成** する
  ため、review session の「一切 checkout を変えない」という原則に反する。
  `--locked --frozen --offline` のようなフラグでも target/ には書くため、
  review session 内では例外なく禁止する (settings.json の deny list も
  同じ立場で cargo 系を全て block している)。
  lint / test の動作を実際に走らせて確認したい場合は、
  **review を一旦終えて別 session で実行** してもらうようユーザーに依頼
  するか、既存の CI 実行結果 (GitHub Actions 等) を参照する。
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
- layered defense: settings.json の `permissions.deny` でブロックされるが、
  skill 側でも明示的に禁止することで 2 重に担保する

## 安全境界の正直な話 (重要)

`settings.json` の `permissions.deny` は **best-effort の layered defense**
であり、**最終的な safety guarantee ではない**。deny list は「コマンド名
ベース」の block list であるため、以下のような interpreter / shell 経由の
書き込みを **網羅的には** 防げない:

- `python -c 'open("f", "w").write(...)'`
- `node -e 'fs.writeFileSync(...)'`
- `sh -c 'printf x > f'`
- `ruby -e 'File.write(...)'`
- ここに並べていない未知の interpreter / custom binary 経由の書き込み

一般的なエントリポイント (python / node / ruby / perl / sh -c / bash -c /
xargs / eval / source 等) は deny list に追加してあるが、**列挙は網羅
ではない**。本物の safety は以下 3 層で担保する:

1. **本 skill の明示的禁止** (これを読んでいる貴方の遵守)
2. `settings.json` の deny list (best-effort block)
3. (将来) vibepod container 側の read-only マウント / overlay 隔離

したがって **interpreter を経由した書き込みも禁止** である。
「deny list に乗っていないから使える」という判断は禁則。疑わしい操作は
実行しない、という原則を貴方が守ること。deny list はあくまで事故防止の
2 次防御であり、1 次防御は貴方の discipline。

## 禁則 (追加)

- `python -c` / `node -e` / `ruby -e` / `perl -e` / `sh -c` 等、
  interpreter にコード文字列を渡す呼び出しは禁止 (read-only な用途で
  あっても、ファイル書き込みが可能なので 2 次防御を抜ける)
- `xargs` / `tee` / `eval` / `source` 等の間接実行も禁止
- 未知の binary を実行するのも禁止 (何をするか保証できないため)

## Self check

操作する前に、自分に問う:

- [ ] これは read-only か？
- [ ] ファイル内容が変わるか？変わるなら禁止
- [ ] git state / 作業ツリー / パッケージ状態が変わるか？変わるなら禁止
- [ ] ユーザーに見える副作用があるか？あるなら禁止（出力・コメント以外）

迷ったら実行しない。指摘に変換する。
