# VibePod 開発ルール

## コードフォーマット

- コミット前に必ず `cargo fmt` を実行すること。CI で `cargo fmt --check` が走るため、フォーマットが崩れていると CI が落ちる
- `cargo clippy` の警告も解消すること
