//! 非 TTY 環境でビルド済みバイナリを実際に起動し、TTY 検出漏れによる
//! クラッシュ（`IO error: not a terminal` 等の dialoguer 生エラーで落ちる）が
//! 無いことを検証する。
//!
//! 背景（Issue #79 C）: `restore.rs` の `ensure_interactive` / `login.rs` の
//! `ensure_interactive_terminal` は純関数として単体テスト済みだが、それらは
//! 「判定ロジックが正しく実装されていること」しか保証しない。判定に**到達する
//! 前**に別の経路で `Term::stderr()` 等が呼ばれれば、判定自体が正しくても
//! 同じクラッシュが起きる。実際 1.9.0 の修正はこのパターンだった。
//! ユニットテストでは検出できず、実バイナリを非 TTY で起動して初めて検出
//! できるため、このファイルを追加する。
//!
//! **scope**: `restore` / `login` は TTY 判定が docker/認証処理より前で
//! 行われる設計（`ensure_interactive` はコマンドの最初の一手、
//! `ensure_interactive_terminal` は `DockerRuntime::new()` より前）ため、
//! docker が無い CI でも決定的に通る。`init` / `run` は docker や認証に
//! 到達しうるためこのファイルでは扱わない（docker 実機テスト側の担当範囲）。

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn vibepod_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vibepod"))
}

/// stdin/stdout/stderr をすべて `Stdio::piped()` にしてバイナリを起動する。
///
/// 3 つ全てをパイプにすることで、`cargo test` をどんな端末から実行しても
/// （手元のインタラクティブシェルからでも、CI の非 TTY からでも）子プロセス
/// から見た stdin/stdout/stderr は常に非 TTY になることを保証する。stdin は
/// 何も書き込まずにハンドルを drop するため、子プロセスからは即座に EOF が
/// 見える（対話プロンプトが来ても入力を待ち続けて test がハングすることは
/// ない）。
fn run_non_tty(args: &[&str], current_dir: &std::path::Path) -> Output {
    let child = Command::new(vibepod_bin())
        .args(args)
        .current_dir(current_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn vibepod binary");
    // `wait_with_output` は内部で stdin を先に drop してから stdout/stderr を
    // 読み切る（何も書き込まないので子プロセスからは即座に EOF が見える）。
    // そのため対話プロンプトが来ても入力待ちで test がハングすることはない。
    child
        .wait_with_output()
        .expect("failed to wait for vibepod binary")
}

#[test]
fn version_flag_succeeds_over_non_tty_stdio() {
    // 回帰の土台: 非 TTY でも --version 自体はクラッシュしない基本契約を
    // 固定する。以降の restore/login のテストは「TTY 判定が正しく機能して
    // いること」を検証するが、その前提としてバイナリ起動自体が非 TTY で
    // 壊れていないことをまず確認する。
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let output = run_non_tty(&["--version"], tmp.path());

    assert!(
        output.status.success(),
        "vibepod --version must exit 0 over non-tty stdio, got status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("vibepod"),
        "expected version output to mention the binary name, got: {}",
        stdout
    );
}

#[test]
fn help_flag_succeeds_over_non_tty_stdio() {
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let output = run_non_tty(&["--help"], tmp.path());

    assert!(
        output.status.success(),
        "vibepod --help must exit 0 over non-tty stdio, got status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage"),
        "expected help output to contain usage information, got: {}",
        stdout
    );
}

#[test]
fn restore_over_non_tty_stdio_fails_with_explicit_terminal_error_not_a_dialoguer_crash() {
    // `restore.rs::execute` は先頭で `ensure_interactive` を呼び、stderr が
    // TTY でなければ即座に bail する（git repo チェックより前）。そのため
    // 実際に git リポジトリを用意する必要はなく、tempdir で決定的に検証できる。
    //
    // ここで検証したいのは「判定ロジックが正しい」ことではなく（それは
    // `ensure_interactive` の単体テストが既にカバーしている）、「判定へ
    // 到達する前に dialoguer の生エラーで落ちていないか」。もし判定より前の
    // どこかで `Select`/`Confirm` 等が非 TTY のまま呼ばれていれば、
    // stderr には `ensure_interactive` のメッセージではなく dialoguer の
    // 生の IO エラーが出るはずで、そちらを検出することが本テストの目的。
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let output = run_non_tty(&["restore"], tmp.path());

    assert!(
        !output.status.success(),
        "vibepod restore over non-tty stdio must fail (exit non-zero), got status={:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    // 文言そのものを固定しない: 「terminal」という語が含まれていることだけを
    // 確認する（運用者が「TTY が要る」と分かればよい。文言変更のたびに
    // テストが壊れるのは避けたい）。
    assert!(
        stderr.to_lowercase().contains("terminal"),
        "expected an explicit terminal-requirement message on stderr, got: {}",
        stderr
    );

    // dialoguer 由来の生エラー（1.7.x/1.9.0 で実際に露出したクラッシュの
    // シグネチャ、`dialoguer::Error` の Display が "IO error: {0}" で
    // "not a terminal" という io::Error メッセージを包んだもの）が出ていない
    // ことを確認する。判定より前に dialoguer が非 TTY のまま呼ばれていれば、
    // ここに引っかかる。
    //
    // 注意: `ensure_interactive` 自身の正常なメッセージ末尾にも
    // "...but stderr is not a terminal." のように「not a terminal」という
    // 語句が偶然含まれるため、"not a terminal" 単体では判定できない。
    // dialoguer の生エラーにしか出ない "IO error:" プレフィックスの有無で
    // 判定する。
    assert!(
        !stderr.contains("IO error:"),
        "raw dialoguer IO error leaked instead of the explicit ensure_interactive message: {}",
        stderr
    );
    assert!(
        !stderr.contains("panicked"),
        "vibepod restore must not panic over non-tty stdio, got: {}",
        stderr
    );
}

#[test]
fn login_over_non_tty_stdio_fails_with_explicit_terminal_error_not_a_dialoguer_crash() {
    // `login.rs::execute` は先頭のバナー出力の直後、`DockerRuntime::new()`
    // （docker デーモンへの接続）より前に `ensure_interactive_terminal` を
    // 呼ぶ。stdin/stderr の両方が非 TTY ならここで即座に bail するため、
    // docker が起動していない環境でも決定的に完走する。
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let output = run_non_tty(&["login"], tmp.path());

    assert!(
        !output.status.success(),
        "vibepod login over non-tty stdio must fail (exit non-zero), got status={:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.to_lowercase().contains("terminal"),
        "expected an explicit terminal-requirement message on stderr, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("IO error:"),
        "raw dialoguer IO error leaked instead of the explicit ensure_interactive_terminal \
         message: {}",
        stderr
    );
    assert!(
        !stderr.contains("panicked"),
        "vibepod login must not panic over non-tty stdio, got: {}",
        stderr
    );

    // docker への到達を検知するための保険: DockerRuntime::new() が実際に
    // 呼ばれてしまった場合に典型的に出るエラー文言が出ていないことを確認する
    // （出ていれば TTY チェックより後に docker 到達している = 制約違反）。
    assert!(
        !stderr.contains("Docker is not running"),
        "login must fail on the TTY check before reaching Docker, got: {}",
        stderr
    );
}
