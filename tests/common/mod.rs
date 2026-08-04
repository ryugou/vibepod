//! integration テスト間で共有するヘルパー。
//!
//! ファイルパスを `tests/common/mod.rs` にしているのは、Rust の慣例で
//! `tests/<name>.rs` は cargo が独立したテストバイナリとしてビルドするが
//! `tests/<name>/mod.rs` は対象外になるため。`tests/common.rs` のような
//! 通常の `.rs` ファイルにすると、テストを1つも持たない空バイナリとして
//! ビルド・リンクされてしまい、ビルド時間が無駄に伸びる。
//!
//! 現在の利用元:
//! - `read_repo_file` / `repo_root`: リポジトリ内の任意ファイルをリポジトリ
//!   ルートからの相対パスで読む共通ヘルパー（Q1: フル再レビュー後の
//!   simplify 指摘で、`read_dockerfile` と
//!   `timeout_workspace_preservation_test.rs` の同型実装を統合した）。
//! - `read_dockerfile`: `dockerfile_codex_pin_test.rs`,
//!   `dockerfile_profile_stage_test.rs`（いずれも `templates/Dockerfile` の
//!   テキストに対する静的アサーションのため、Dockerfile 読み込みを共有する）。
//!   `read_repo_file("templates/Dockerfile")` を呼ぶだけの薄いラッパー。
//! - `init_test_repo`: `git_test.rs`, `integration_test.rs`（いずれもコミット
//!   1件を持つ最小限の git リポジトリを前提にしたテストのため、
//!   セットアップ処理を共有する。F10: フル再レビュー指摘で
//!   `integration_test.rs` に重複していたインライン実装を統合した）。
//!
//! `mod common;` は各 integration テストバイナリごとに独立にコンパイルされる
//! ため、このファイルの関数のうち呼び出し側が使わない方（例:
//! `git_test.rs` は `read_dockerfile` を使わない）は、そのバイナリの中では
//! 未使用として `dead_code` 警告の対象になる。これは `tests/common/mod.rs`
//! 方式の既知のトレードオフ（各バイナリが共有ヘルパーの部分集合だけを使う）
//! であり、関数自体が本当に不要という意味ではないため、モジュール全体に
//! `#![allow(dead_code)]` を付けて抑制する。

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// リポジトリルート（`Cargo.toml` があるディレクトリ）を返す。
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// リポジトリルートからの相対パス `relative_path` のファイルを文字列として
/// 読み込む。
///
/// 読み込みに失敗した場合はパスを含めてパニックする — テストの実行環境
/// (リポジトリ直下でない、ファイルが無い等)の問題を、アサーション失敗
/// ではなく明確なエラーとして区別するため。
pub fn read_repo_file(relative_path: &str) -> String {
    let path = repo_root().join(relative_path);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// `templates/Dockerfile` を文字列として読み込む（`read_repo_file` の薄い
/// ラッパー）。
pub fn read_dockerfile() -> String {
    read_repo_file("templates/Dockerfile")
}

/// テスト用の最小限の git リポジトリを一時ディレクトリに作成する。
/// `git init` → user.email/user.name 設定 → 空コミット1件、の順で行う。
///
/// 空コミット（`--allow-empty`）にしているのは、これを使うテストの大半が
/// 「有効な HEAD が存在すること」だけを前提としており、コミット内容自体は
/// 使わないため。実ファイルを書いてコミットする必要があるテストは、返した
/// `TempDir` の中で追加のファイル作成・コミットを行えばよい。
pub fn init_test_repo() -> TempDir {
    let dir = TempDir::new().expect("failed to create test repo tempdir");
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run git init");
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run git config user.email");
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run git config user.name");
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run git commit --allow-empty");
    dir
}
