//! integration テスト間で共有するヘルパー。
//!
//! ファイルパスを `tests/common/mod.rs` にしているのは、Rust の慣例で
//! `tests/<name>.rs` は cargo が独立したテストバイナリとしてビルドするが
//! `tests/<name>/mod.rs` は対象外になるため。`tests/common.rs` のような
//! 通常の `.rs` ファイルにすると、テストを1つも持たない空バイナリとして
//! ビルド・リンクされてしまい、ビルド時間が無駄に伸びる。
//!
//! 現在の利用元: `dockerfile_codex_pin_test.rs`,
//! `dockerfile_profile_stage_test.rs`（いずれも `templates/Dockerfile` の
//! テキストに対する静的アサーションのため、Dockerfile 読み込みを共有する）。

use std::fs;
use std::path::PathBuf;

/// `templates/Dockerfile` を文字列として読み込む。
///
/// 読み込みに失敗した場合はパスを含めてパニックする — テストの実行環境
/// (リポジトリ直下でない、ファイルが無い等)の問題を、アサーション失敗
/// ではなく明確なエラーとして区別するため。
pub fn read_dockerfile() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/Dockerfile");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}
