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
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn vibepod_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vibepod"))
}

/// `run_non_tty` が子プロセスの終了を待つ上限。TTY 判定は即座に bail する
/// 現在の実装に対しては十分すぎるほど余裕がある（実測は数十ミリ秒オーダー）。
const NON_TTY_TIMEOUT: Duration = Duration::from_secs(30);

/// `child` の終了を `timeout` まで待つ。待ちきれない場合は kill した上で、
/// どのコマンドがハングしたかを含むメッセージで panic する。
///
/// 現在の `run_non_tty` は「stdin に何も書かず EOF を送る」ことで TTY 判定を
/// 即座に bail させる前提に依存している。将来 TTY 判定より前に「EOF を見ない
/// 待機処理」（例: 何らかの応答を待つ処理）が挟まると、この前提が崩れて
/// 子プロセスが無言で待ち続け、CI のジョブ timeout（15分）まで原因不明の
/// まま止まる。ここで明示的に timeout を切り、パニックメッセージにコマンド名
/// を含めることで、CI ログから即座にハング箇所を特定できるようにする。
///
/// 実装は標準ライブラリのみを使う（新規依存を追加しない）: 別スレッドで
/// `wait_with_output`（stdin を drop してから stdout/stderr を読み切る）を
/// 実行し、`mpsc::Receiver::recv_timeout` で待つ。タイムアウトした場合は
/// `child.id()` で控えておいた pid に対し OS の `kill` コマンドで SIGKILL を
/// 送る（`Child::kill()` は `wait_with_output` に所有権を渡した別スレッド側
/// にあるため、こちらのスレッドからは呼べない）。
fn wait_with_timeout(child: Child, timeout: Duration, description: &str) -> Output {
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        // 受信側が timeout で先に panic して drop されている場合、send は
        // Err を返すだけで無視してよい（プロセス自体は下の timeout 分岐で
        // 既に kill 済み）。
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => {
            result.unwrap_or_else(|e| panic!("failed to wait for `{}`: {}", description, e))
        }
        Err(_) => {
            // kill の成否そのものは本質ではない（既に終了していれば失敗して
            // 当然）。目的は次の panic で原因箇所を伝えることなので結果は
            // 握りつぶしてよい。
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            panic!(
                "`{}` (pid={}) did not finish within {:?} — timeout in \
                 non_tty_test.rs::wait_with_timeout. This means the process hung instead of \
                 exiting on stdin EOF; check for new blocking I/O before the TTY check.",
                description, pid, timeout
            );
        }
    }
}

/// stdin/stdout/stderr をすべて `Stdio::piped()` にしてバイナリを起動する。
///
/// 3 つ全てをパイプにすることで、`cargo test` をどんな端末から実行しても
/// （手元のインタラクティブシェルからでも、CI の非 TTY からでも）子プロセス
/// から見た stdin/stdout/stderr は常に非 TTY になることを保証する。stdin は
/// 何も書き込まずにハンドルを drop するため、子プロセスからは即座に EOF が
/// 見える（対話プロンプトが来ても入力を待ち続けて test がハングすることは
/// ない）。万一 EOF だけでは終了しない経路が将来紛れ込んでも、
/// `wait_with_timeout` が `NON_TTY_TIMEOUT` で打ち切る。
fn run_non_tty(args: &[&str], current_dir: &std::path::Path) -> Output {
    let child = Command::new(vibepod_bin())
        .args(args)
        .current_dir(current_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn vibepod binary");

    wait_with_timeout(
        child,
        NON_TTY_TIMEOUT,
        &format!("vibepod {}", args.join(" ")),
    )
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

#[test]
fn wait_with_timeout_kills_hung_process_and_panics_with_description() {
    // `run_non_tty` が想定する「stdin EOF だけで終了する」前提が将来崩れた
    // ケースを模す: `sleep 30` は EOF を見ても終了しない。timeout を短く
    // 切って、実際にこの分岐を通ることを確認する（本物の vibepod バイナリの
    // NON_TTY_TIMEOUT=30秒を毎回待つと test 自体が遅くなるため使わない）。
    let child = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sleep(1) test double");

    let started = std::time::Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_with_timeout(child, Duration::from_millis(200), "sleep 30 (test double)")
    }));
    let elapsed = started.elapsed();

    let panic_payload = result.expect_err("wait_with_timeout must panic when the process hangs");
    let message = panic_payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("panic payload should carry a string message");

    assert!(
        message.contains("sleep 30 (test double)"),
        "panic message should name the command that hung so operators know where to look, got: {}",
        message
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "wait_with_timeout should return promptly once the deadline passes instead of waiting \
         for the natural exit, took {:?}",
        elapsed
    );
}

#[test]
fn wait_with_timeout_returns_output_when_process_finishes_before_deadline() {
    let child = Command::new("true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn true(1) test double");

    let output = wait_with_timeout(child, Duration::from_secs(5), "true (test double)");

    assert!(
        output.status.success(),
        "expected the fast-exiting test double to succeed, got status={:?}",
        output.status
    );
}
