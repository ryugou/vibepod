# vibepod restore 設計仕様

## 概要

`vibepod restore` は、エージェントの作業を完全に元に戻しつつ、何をやったかのレポートを残すコマンド。次回リトライ時にレポートを参考にして改善できる。

## データフロー

```
vibepod run 開始
  → .vibepod/ が未作成なら作成 + .gitignore に追加
  → HEAD ハッシュを .vibepod/sessions.json に記録
  → エージェント作業...
  → 終了

vibepod restore
  → sessions.json から実行履歴を一覧表示（dialoguer で選択）
  → 選択した実行の開始HEAD〜現在HEADの差分からレポート生成
  → .vibepod/reports/YYYY-MM-DD-HHMMSS.md に保存
  → 削除対象の未追跡ファイル一覧を表示して確認
  → git reset --hard <開始HEAD> + git clean -fd
  → セッションレコードに restored: true を付与
  → 完了メッセージ
```

## セッション記録

### 記録タイミング

`vibepod run` の実行開始時に `.vibepod/sessions.json` へ1レコード追加する。

### .vibepod/ ディレクトリの初期化

`vibepod run` で初回セッション記録時に:

1. `.vibepod/` ディレクトリをプロジェクトルートに作成
2. `.vibepod/reports/` ディレクトリを作成
3. `.gitignore` に `.vibepod/` が含まれていなければ追加

`restore` 実行前に `.vibepod/` が git にトラッキングされていないことを確認し、トラッキングされている場合はエラーとする。

### sessions.json の構造

```json
{
  "sessions": [
    {
      "id": "20260323-120000-a3f2",
      "started_at": "2026-03-23T12:00:00+09:00",
      "head_before": "abc1234",
      "branch": "main",
      "prompt": "--resume",
      "claude_session_path": "~/.claude/projects/.../session.jsonl",
      "restored": false
    }
  ]
}
```

- `id`: タイムスタンプ + ランダム suffix の一意識別子（例: `20260323-120000-a3f2`）
- `started_at`: 実行開始日時（JST）
- `head_before`: 実行開始時の HEAD コミットハッシュ（フルハッシュ）
- `branch`: 実行開始時のブランチ名
- `prompt`: `--prompt` の値、`--resume`、または `"interactive"`
- `claude_session_path`: Claude Code のセッションログへのファイルパス。取得できない場合は `null`
- `restored`: restore 済みかどうか

### 件数制限

sessions.json は直近100件を保持する。100件を超えた古いレコードは記録時に自動削除する。

### claude_session_path の取得

Claude Code のセッションログは `~/.claude/projects/<project-path-hash>/` 配下に保存される。`vibepod run` 開始時にこのパスの特定を試みるが、取得できない場合は `null` とする（`Option<String>`）。

## レポート

### 保存先

`.vibepod/reports/YYYY-MM-DD-HHMMSS.md`

### レポート構造

```markdown
# VibePod Session Report

- **実行日時:** 2026-03-23 12:00:00 JST
- **ブランチ:** main
- **モード:** interactive
- **開始HEAD:** abc1234
- **終了HEAD:** def5678
- **Claude セッションログ:** ~/.claude/projects/.../session.jsonl

## コミット一覧

- def5678 feat: add login page
- ccc4444 fix: update styles
- bbb3333 refactor: extract component

## 変更ファイル一覧

- A src/pages/login.rs (新規)
- M src/main.rs (変更)
- D src/old_module.rs (削除)

## 変更統計

 src/pages/login.rs | 45 +++++++++++++++
 src/main.rs        |  3 +-
 src/old_module.rs  | 30 ----------
 3 files changed, 46 insertions(+), 32 deletions(-)
```

- diff 全文は含めない（膨大になるため）
- 必要なら `git diff <開始HEAD>..<終了HEAD>` で確認可能

## CLI インターフェース

```
vibepod restore
```

- 引数なし
- `.vibepod/sessions.json` から過去の実行履歴を一覧表示（`restored: true` のセッションは除外）
- dialoguer で選択（直近が一番上）
- 確認プロンプトで削除対象の未追跡ファイル一覧を表示
- 選択したセッションより後のセッションがある場合「以降の全変更が巻き戻されます」と警告
- レポート生成 → reset → 完了

### CLI 出力イメージ

```
  ┌  VibePod Restore
  │
  ◆  どのセッションに戻しますか？
  │  ● 2026-03-23 12:00 (main) abc1234 - interactive
  │  ○ 2026-03-22 15:30 (main) 789beef - --prompt "fix bug"
  │  ○ 2026-03-22 10:00 (feat/x) 456dead - --resume
  │
  ⚠  このセッション以降の全ての変更が巻き戻されます。
  │
  │  以下の未追跡ファイルも削除されます:
  │    src/pages/login.rs
  │    src/components/new_widget.rs
  │
  ◆  続行しますか？ (y/N)
  │
  ◇  レポートを保存しました: .vibepod/reports/2026-03-23-120000.md
  │
  ◇  git reset --hard abc1234
  │  git clean -fd
  │
  ◇  復元完了！
  └
```

## リセット方式

`git reset --hard <開始HEAD>` + `git clean -fd` を使用。

- `git reset --hard`: コミット、ステージング、作業ツリーの変更を全て元に戻す
- `git clean -fd`: 未追跡のファイル・ディレクトリを削除（`.gitignore` 対象は除外されるため `.vibepod/` は安全）
- 余計なブランチを残さずシンプルに「なかったことにする」

## エラーハンドリング

| 状況 | 挙動 |
|------|------|
| `.vibepod/sessions.json` が存在しない | 「セッション履歴がありません」 |
| 作業ツリーにコミットされていない変更がある | 「未コミットの変更があります。先にコミットするか stash してください」で中止 |
| 選択したセッション以降に変更がない（HEAD が同じ） | 「変更がありません」 |
| 記録された開始 HEAD が存在しない（rebase, gc, shallow clone 等） | 「コミット abc1234 が見つかりません」で中止 |
| git リポジトリ外で実行 | 「git リポジトリ内で実行してください」 |
| 記録時と異なるブランチにいる | 「ブランチが異なります。続行しますか？」と確認 |
| 開始 HEAD が現在 HEAD の祖先でない（手動 hard reset、force push 等） | 「セッション開始時点のコミットが現在のブランチ履歴上にありません。強制的に戻しますか？」と確認 |
| `.vibepod/sessions.json` が壊れている（不正な JSON） | 「セッション履歴ファイルが破損しています」 |
| `.vibepod/` が git にトラッキングされている | 「.vibepod/ が git 管理下にあります。.gitignore に追加してください」で中止 |
| 復元可能なセッションがない（全て `restored: true`） | 「復元可能なセッションがありません」 |

## 変更対象ファイル

### 新規作成
- `src/cli/restore.rs` — restore コマンドの実装
- `src/git.rs` — git 操作ヘルパー（run.rs と restore.rs で共用）

### 変更
- `src/lib.rs` — `git` モジュールの公開
- `src/cli/mod.rs` — `Restore` サブコマンドの追加
- `src/main.rs` — restore コマンドのルーティング
- `src/cli/run.rs` — セッション記録追加 + git 操作をヘルパーに移行

## テスト方針

### ユニットテスト
- sessions.json の読み書き・件数制限
- レポート生成ロジック（コミット一覧、変更ファイル一覧、変更統計の生成）
- セッション ID のユニーク性

### 統合テスト
- `vibepod restore` の CLI パーステスト
- git リポジトリ外での実行エラー
- エラーハンドリングの各パターン
