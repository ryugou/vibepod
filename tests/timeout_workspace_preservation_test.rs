//! 要件2（設計書 第3節: タイムアウト時の workspace 保全）の不変条件に対する
//! 静的ソースガード（フル再レビュー F8、2026-08 追補）。
//!
//! **なぜテキストベースなのか**: 「タイムアウト経路が workspace を変更せず、
//! セッションを restored 扱いにもしない」ことを実際に動かして確認するには、
//! 本来は `run_fire_and_forget`（`src/cli/run/prompt.rs`）を実タイムアウトさせる
//! e2e テストが要る。しかし:
//! - `run_fire_and_forget` は `pub(super)`（`crate::cli::run` 内限定）であり、
//!   外部の `tests/*.rs` から直接呼べない。呼べるようにするには可視性を
//!   `pub` へ上げる API 変更が必要で、これはテスト1件のコストとしては
//!   不釣り合いに大きい。
//! - 実際にタイムアウトさせるには Docker イメージビルド・コンテナ起動・
//!   短い `prompt_idle_timeout` での実行が要り、このリポジトリには
//!   （`docker_test.rs` の `#[ignore]` テスト群を含めても）この粒度の
//!   e2e ハーネスの前例が無い。
//!
//! そのため、可視性変更や新規 e2e 基盤の追加ではなく、
//! `dockerfile_codex_pin_test.rs` / `dockerfile_profile_stage_test.rs` と同じ
//! 確立済みの様式（ソーステキストへの静的アサーション）を採用する。
//!
//! **限界（正直な注記）**: これは「実際にタイムアウトさせて workspace が
//! 変更されないこと」を機能的に証明するものではない。あくまで「タイムアウト
//! 処理の該当コードパスが、workspace を変更する API
//! （`git::reset_hard` / `git::clean_fd`）や `mark_restored` への参照、および
//! `git` を直接起動する生の `Command::new("git")` 呼び出しを、ソースコード上に
//! 一切持たない」ことを固定するに留まる。誰かが将来 `if was_timed_out`
//! ブロックへ git 変異操作を再混入させたときに `cargo test`（秒単位）で
//! それを検知できることが目的であり、real docker run による e2e 検証の
//! 代替ではない。
//!
//! **`Command::new("git")` も禁止する理由（mn3、フル再レビュー指摘）**:
//! この要件2対応で削除した旧実装は `git::reset_hard` のような名前付き
//! ヘルパーではなく、`Command::new("git").args(["reset", "--mixed", ...])`
//! という生の `git` 起動だった。`reset_hard` / `clean_fd` / `mark_restored`
//! という関数名だけを禁止リストにしていると、将来同じ種類の git 変異操作が
//! 名前付きヘルパーを経由せず `Command::new("git")` で直接書き戻されても
//! この静的ガードは検知できない。`prompt.rs` は元々 `docker` しか起動しない
//! （git 操作は全て `crate::git` 経由）ため、`Command::new("git")` の不在も
//! あわせて固定する。
//!
//! 過剰検知（コメント中の言及だけで red になる等）は許容し、むしろ意図的に
//! 単純な文字列検索にしている。誤検知が実際に起きたら、そのときに初めて
//! 判定を精緻化する（先回りして複雑にしない）。

mod common;

use common::{read_repo_file, repo_root};
use std::fs;
use std::path::{Path, PathBuf};

/// `dir` 配下の `.rs` ファイルを再帰的に列挙する。
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("failed to read dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("failed to read a directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn timeout_path_has_no_workspace_mutating_git_calls() {
    // 要件2の核心: タイムアウト処理本体（`if was_timed_out` ブロックを含む
    // `run_fire_and_forget`）は、workspace の git 状態を変更する API を
    // 一切呼ばない。`git::reset_hard` / `git::clean_fd` は workspace を
    // 直接変更する関数、`mark_restored` はセッションを「復元済み」扱いに
    // して `vibepod restore` の対象から外す関数で、いずれもタイムアウト時に
    // 呼ばれてはならない。
    let prompt_rs = read_repo_file("src/cli/run/prompt.rs");
    for forbidden in [
        "reset_hard",
        "clean_fd",
        "mark_restored",
        "Command::new(\"git\")",
    ] {
        assert!(
            !prompt_rs.contains(forbidden),
            "src/cli/run/prompt.rs must not reference `{forbidden}` — this would violate 要件2 \
             (the timeout path must never mutate the workspace or mark the session as \
             restored). Check whether a git-mutating call (whether via a named helper like \
             `git::reset_hard`, or a raw `Command::new(\"git\")` invocation — the historical \
             regression form, mn3) or `mark_restored` was reintroduced into the \
             `if was_timed_out` block or elsewhere in this file."
        );
    }
}

#[test]
fn mark_restored_is_only_referenced_from_restore_and_session() {
    // `mark_restored` の定義元（session.rs）と唯一の正当な呼び出し元
    // （restore.rs、`vibepod restore` コマンド本体）以外に参照が無いことを
    // src/ 全体を走査して固定する。新しい呼び出し元が正当に必要になった
    // 場合は、ここに黙って追加するのではなく `allowed` リストへ明示的に
    // 追記すること（レビューで気づけるようにするため）。
    let root = repo_root().join("src");
    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);

    let allowed: [PathBuf; 2] = [root.join("session.rs"), root.join("cli").join("restore.rs")];

    for file in &files {
        let content = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));
        if content.contains("mark_restored") {
            assert!(
                allowed.contains(file),
                "unexpected `mark_restored` reference in {} — only src/session.rs (definition) \
                 and src/cli/restore.rs (the `vibepod restore` command) may reference it. If a \
                 new legitimate caller is genuinely needed, add it to the `allowed` list in this \
                 test deliberately instead of just widening this check to make it pass.",
                file.display()
            );
        }
    }

    // 対称性チェック: `allowed` に挙げた2ファイルが実際に存在し、
    // `mark_restored` を含んでいることも確認する。session.rs や
    // restore.rs から参照が消えたのにこのテストだけ green のまま残る
    // ような片手落ちを防ぐ。
    for expected in &allowed {
        let content = fs::read_to_string(expected)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", expected.display()));
        assert!(
            content.contains("mark_restored"),
            "expected {} to reference `mark_restored`; if this is no longer true, update the \
             `allowed` list and this assertion together",
            expected.display()
        );
    }
}
