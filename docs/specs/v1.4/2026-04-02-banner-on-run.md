# vibepod run 起動時に ASCII アートバナーを表示する

## 背景

現在 `vibepod run` の起動時は `┌  VibePod` というテキストヘッダーのみ表示している。
`vibepod init` では `src/ui/banner.rs` の ASCII アートバナーを表示しているが、`run` では使われていない。
起動時の見栄えを改善するため、`run` でも AA バナーを表示する。

## 変更対象

- `src/cli/run.rs`

## 変更内容

1. `use crate::ui::banner;` を import セクションに追加
2. L135 `println!("\n  ┌  VibePod");` の直前に `banner::print_banner();` を呼ぶ
3. `println!("\n  ┌  VibePod");` を `println!("  ┌");` に変更（AA でタイトルが出るため重複排除）

## 変更後の出力イメージ

```
 ██╗   ██╗██╗██████╗ ███████╗██████╗  ██████╗ ██████╗
 ██║   ██║██║██╔══██╗██╔════╝██╔══██╗██╔═══██╗██╔══██╗
 ██║   ██║██║██████╔╝█████╗  ██████╔╝██║   ██║██║  ██║
 ╚██╗ ██╔╝██║██╔══██╗██╔══╝  ██╔═══╝ ██║   ██║██║  ██║
  ╚████╔╝ ██║██████╔╝███████╗██║     ╚██████╔╝██████╔╝
   ╚═══╝  ╚═╝╚═════╝ ╚══════╝╚═╝      ╚═════╝ ╚═════╝

  ┌
  ◇  Detected git repository: recording-control
  │  Remote: https://github.com/ryugou/recording-control.git
  │  Branch: main
  │
  ◇  Starting container...
  │  Agent: Claude Code
  │  Mode: interactive
  │  Mount: /path/to/project → /workspace
  │
  ◇  Container: vibepod-recording-control-xxxxx
  └
```

## 影響範囲

- `vibepod run` のみ。`login` / `logout` / `restore` は変更なし
- 既存の `banner::print_banner()` をそのまま使うため、AA 自体の変更はなし
