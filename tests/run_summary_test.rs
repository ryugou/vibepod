//! `--timeout` パースと `--prompt` 実行後の要約レンダリング（純関数）の検証。

use vibepod::cli::run::{
    parse_timeout_secs, render_run_summary, render_timeout_message, DEFAULT_OVERALL_TIMEOUT_SECS,
};
use vibepod::git::ChangedFiles;

/// Shorthand: a successfully-computed changed-file list.
fn computed(files: &[&str]) -> ChangedFiles {
    ChangedFiles::Computed(files.iter().map(|s| s.to_string()).collect())
}

// --- parse_timeout_secs ---

#[test]
fn timeout_bare_seconds() {
    assert_eq!(parse_timeout_secs("1800").unwrap(), 1800);
}

#[test]
fn timeout_zero_means_disabled() {
    assert_eq!(parse_timeout_secs("0").unwrap(), 0);
}

#[test]
fn timeout_duration_minutes() {
    assert_eq!(parse_timeout_secs("30m").unwrap(), 30 * 60);
}

#[test]
fn timeout_duration_compound() {
    assert_eq!(parse_timeout_secs("1h30m").unwrap(), 90 * 60);
}

#[test]
fn timeout_duration_seconds_suffix() {
    assert_eq!(parse_timeout_secs("90s").unwrap(), 90);
}

#[test]
fn timeout_trims_whitespace() {
    assert_eq!(parse_timeout_secs("  45m ").unwrap(), 45 * 60);
}

#[test]
fn timeout_empty_is_error() {
    assert!(parse_timeout_secs("   ").is_err());
}

#[test]
fn timeout_garbage_is_error() {
    let err = parse_timeout_secs("soon").unwrap_err();
    assert!(
        err.to_string().contains("invalid --timeout"),
        "expected actionable error, got: {}",
        err
    );
}

#[test]
fn timeout_default_is_thirty_minutes() {
    assert_eq!(DEFAULT_OVERALL_TIMEOUT_SECS, 30 * 60);
}

// --- render_run_summary ---

#[test]
fn summary_success_lists_status_and_logs() {
    let changed = computed(&["src/main.rs", "README.md"]);
    let out = render_run_summary(
        true,
        "success",
        Some("All checks pass."),
        &changed,
        "/w/.vibepod/sessions/abc/logs.txt",
    );
    assert!(out.contains("Status: success"), "got: {}", out);
    assert!(out.contains("Result: All checks pass."), "got: {}", out);
    assert!(out.contains("Changed files (2):"), "got: {}", out);
    assert!(out.contains("src/main.rs"), "got: {}", out);
    assert!(out.contains("README.md"), "got: {}", out);
    assert!(
        out.contains("Full logs: /w/.vibepod/sessions/abc/logs.txt"),
        "got: {}",
        out
    );
}

#[test]
fn summary_failure_shows_reason() {
    let out = render_run_summary(
        false,
        "error_max_turns",
        None,
        &computed(&[]),
        "/tmp/logs.txt",
    );
    assert!(
        out.contains("Status: failed (error_max_turns)"),
        "got: {}",
        out
    );
}

#[test]
fn summary_no_changes_says_none() {
    let out = render_run_summary(true, "success", None, &computed(&[]), "/tmp/logs.txt");
    assert!(out.contains("Changed files: (none)"), "got: {}", out);
}

#[test]
fn summary_unavailable_is_distinct_from_none() {
    // 算出不能は「変更なし (none)」と別文言で表示され、誤って無変更に
    // 見えないこと（指摘 #2 の不変条件）。
    let out = render_run_summary(
        true,
        "success",
        None,
        &ChangedFiles::Unavailable,
        "/tmp/logs.txt",
    );
    assert!(
        !out.contains("Changed files: (none)"),
        "unavailable must not read as 'none': {}",
        out
    );
    assert!(
        out.contains("could not be computed"),
        "unavailable must be explicit: {}",
        out
    );
}

#[test]
fn summary_omits_empty_result_text() {
    // 空白のみの result 本文は Result 行を出さない。
    let out = render_run_summary(
        true,
        "success",
        Some("   "),
        &computed(&[]),
        "/tmp/logs.txt",
    );
    assert!(!out.contains("Result:"), "got: {}", out);
}

#[test]
fn summary_always_includes_logs_path() {
    // 生ログの保存先は常に提示される（要約が生ログの代替になるため）。
    let out = render_run_summary(
        false,
        "whatever",
        None,
        &computed(&[]),
        "/some/where/logs.txt",
    );
    assert!(
        out.contains("Full logs: /some/where/logs.txt"),
        "got: {}",
        out
    );
}

// --- render_timeout_message ---
//
// 要件2（設計書 第3節）: タイムアウト時は workspace を一切変更しない。この
// 関数は純関数であり git コマンドを一切呼ばないため、呼び出すだけで
// 「git 操作を行わない」ことを構造的に保証できる。ここでは組み立てられる
// 文言が期待する内容（git status / git log / vibepod restore への言及）を
// 含むことを検証する。

#[test]
fn timeout_message_mentions_recovery_steps_not_reset() {
    let out = render_timeout_message(
        false,
        900,
        1800,
        Some(std::path::Path::new("/tmp/logs.txt")),
    );
    assert!(
        out.contains("git status"),
        "must tell the operator how to inspect the workspace: {}",
        out
    );
    assert!(
        out.contains("git log"),
        "must tell the operator how to inspect commits: {}",
        out
    );
    assert!(
        out.contains("vibepod restore"),
        "must point to the manual recovery command: {}",
        out
    );
    assert!(
        !out.contains("reset"),
        "must not claim an automatic reset happened: {}",
        out
    );
}

#[test]
fn timeout_message_idle_uses_idle_limit() {
    let out = render_timeout_message(false, 300, 1800, None);
    assert!(
        out.contains("ストリーム無出力"),
        "idle timeout should be labeled as such: {}",
        out
    );
    assert!(
        out.contains("5 分"),
        "300s idle limit should read as 5 分: {}",
        out
    );
}

#[test]
fn timeout_message_overall_uses_overall_limit() {
    let out = render_timeout_message(true, 300, 1800, None);
    assert!(
        out.contains("実時間"),
        "overall timeout should be labeled as such: {}",
        out
    );
    assert!(
        out.contains("30 分"),
        "1800s overall limit should read as 30 分: {}",
        out
    );
}

#[test]
fn timeout_message_includes_log_path_when_present() {
    let out = render_timeout_message(true, 300, 60, Some(std::path::Path::new("/w/logs.txt")));
    assert!(out.contains("/w/logs.txt"), "got: {}", out);
}

#[test]
fn timeout_message_omits_log_path_when_absent() {
    let out = render_timeout_message(true, 300, 60, None);
    assert!(
        !out.contains("ログ:"),
        "no log line should be printed when no path is available: {}",
        out
    );
}

#[test]
fn timeout_message_under_a_minute_uses_seconds() {
    let out = render_timeout_message(true, 300, 45, None);
    assert!(out.contains("45 秒"), "got: {}", out);
}
