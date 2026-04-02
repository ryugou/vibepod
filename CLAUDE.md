# VibePod 開発ルール

## コードフォーマット

- コミット前に必ず `cargo fmt` を実行すること。CI で `cargo fmt --check` が走るため、フォーマットが崩れていると CI が落ちる
- `cargo clippy` の警告も解消すること

## エラーハンドリング

- `unwrap()` および `expect()` の使用を原則禁止とする。代わりに `?` 演算子や `match`、`if let` を使って適切にエラーを伝播または処理すること
- テストコード内では `unwrap()` / `expect()` の使用を許可する
- どうしても使用する場合（regex コンパイルでパターンがリテラルで正しいことが保証されている場合など）は、パニックしないことが論理的に自明な理由をコメントで明記すること

## レビューフロー

実装が完了したら、以下のレビューフローを実行すること:

### 共通準備
- 現在のブランチが main の場合は `git checkout -b <適切なブランチ名>` で新しいブランチを作成する

### Codex Review（ローカル、コミット前）
以下を指摘がなくなるまで繰り返す（最大 5 回）:
1. Bash ツールで `codex review --dangerously-bypass-approvals-and-sandbox` を実行する
（重要: Claude Code の内蔵レビュー機能やスキルではなく、Codex CLI コマンドを Bash で直接実行すること。コンテナ内で実行するため sandbox は不要）
2. 出力を確認する。「指摘なし」「no issues」等であればこのフェーズ完了
3. 指摘があれば該当箇所を修正する
4. 手順 1 に戻る

### コミットと PR 作成
1. 変更内容をコミットする（Conventional Commits 準拠）
2. `git push -u origin <ブランチ名>` でリモートに push する
3. `gh pr create --base main` で PR を作成する

### 完了
- 最終的な PR の URL を出力する
