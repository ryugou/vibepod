# Worktrees 自動セットアップ設計

## 背景

vibepod で `--prompt` を使って計画を実行する際、superpowers の `executing-plans` / `subagent-driven-development` スキルが `using-git-worktrees` を REQUIRED として呼び出す。

`using-git-worktrees` スキルは以下の優先順で worktree ディレクトリを探す：

1. `.worktrees/` が存在するか
2. `worktrees/` が存在するか
3. CLAUDE.md に worktree ディレクトリの指定があるか
4. ユーザーに聞く

`-p`（`--prompt`）モードでは対話ができないため、ステップ 4 で詰まる。対話モードでは計画作成時に worktree は作られず（計画ファイルを書くだけ）、実行時に初めて worktree が必要になる。

## 解決策

`vibepod run` の起動前処理で `.worktrees/` ディレクトリを自動作成し、`.gitignore` に追加する。

## 変更箇所

### src/cli/run.rs

既存の `.vibepod/` の `.gitignore` 追加処理（L68-87）を拡張する。

- `cwd.join(".worktrees")` が存在しなければ作成（`create_dir_all`）
- `.gitignore` に `.worktrees/` が含まれていなければ追加

### 動作仕様

- `.worktrees/` が既に存在する場合はスキップ（冪等）
- `.gitignore` に既に `.worktrees` がある場合もスキップ
- `.gitignore` が存在しない場合は `.vibepod/` と `.worktrees/` の両方を含めて新規作成
- 全モード共通（interactive / --prompt / --resume / bridge）

### 影響範囲

- 既存プロジェクトでも次回 `vibepod run` 時に自動で `.worktrees/` が作られる
- superpowers スキルはステップ 1 で `.worktrees/` を発見し、ユーザーへの質問をスキップする
- `.worktrees/` は空ディレクトリ。中身は `git worktree add` が生成する
