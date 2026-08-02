//! `--prompt-file` の読み込みロジック（純関数 `resolve_prompt_file`）の検証。
//!
//! 設計書 第4節: ホストシェルの解釈を経由せずプロンプトを渡すための経路。
//! 内容は無加工（trim せず）で返る必要がある — 山括弧・波括弧・バッククォート・
//! `$` を含むファイルでもそのまま `opts.prompt` に入り、`claude -p` へ渡る。

use tempfile::TempDir;
use vibepod::cli::run::resolve_prompt_file;

#[test]
fn special_characters_pass_through_unmodified() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prompt.txt");
    // 山括弧・波括弧・バッククォート・`$` を含む内容。
    let content = "Fix <the> {bug} in `main.rs` and use $HOME correctly.\n";
    std::fs::write(&path, content).unwrap();

    let resolved = resolve_prompt_file(path.to_str().unwrap()).unwrap();

    assert_eq!(
        resolved, content,
        "prompt file content must pass through byte-for-byte, no trimming or shell interpretation"
    );
}

#[test]
fn leading_and_trailing_whitespace_is_preserved() {
    // 「無加工」要件: trim しないことを明示的に確認する。
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prompt.txt");
    let content = "  padded content  \n\n";
    std::fs::write(&path, content).unwrap();

    let resolved = resolve_prompt_file(path.to_str().unwrap()).unwrap();

    assert_eq!(resolved, content);
}

#[test]
fn nonexistent_file_is_an_error_with_path() {
    let missing = "/nonexistent/path/does-not-exist-vibepod-test.txt";
    let err = resolve_prompt_file(missing).unwrap_err();
    assert!(
        err.to_string().contains(missing),
        "error should include the path so operators can act on it, got: {}",
        err
    );
}

#[test]
fn empty_file_is_an_error_with_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.txt");
    std::fs::write(&path, "").unwrap();

    let err = resolve_prompt_file(path.to_str().unwrap()).unwrap_err();
    assert!(
        err.to_string().contains(path.to_str().unwrap()),
        "error should include the path, got: {}",
        err
    );
}

#[test]
fn whitespace_only_file_is_an_error_with_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("whitespace.txt");
    std::fs::write(&path, "   \n\t \n").unwrap();

    let err = resolve_prompt_file(path.to_str().unwrap()).unwrap_err();
    assert!(
        err.to_string().contains(path.to_str().unwrap()),
        "error should include the path, got: {}",
        err
    );
}
