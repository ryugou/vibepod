//! `--timeout` パースと `--prompt` 実行後の要約レンダリング（純関数）の検証。

use vibepod::cli::run::{parse_timeout_secs, render_run_summary, DEFAULT_OVERALL_TIMEOUT_SECS};

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
    let changed = vec!["src/main.rs".to_string(), "README.md".to_string()];
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
    let out = render_run_summary(false, "error_max_turns", None, &[], "/tmp/logs.txt");
    assert!(
        out.contains("Status: failed (error_max_turns)"),
        "got: {}",
        out
    );
}

#[test]
fn summary_no_changes_says_none() {
    let out = render_run_summary(true, "success", None, &[], "/tmp/logs.txt");
    assert!(out.contains("Changed files: (none)"), "got: {}", out);
}

#[test]
fn summary_omits_empty_result_text() {
    // 空白のみの result 本文は Result 行を出さない。
    let out = render_run_summary(true, "success", Some("   "), &[], "/tmp/logs.txt");
    assert!(!out.contains("Result:"), "got: {}", out);
}

#[test]
fn summary_always_includes_logs_path() {
    // 生ログの保存先は常に提示される（要約が生ログの代替になるため）。
    let out = render_run_summary(false, "whatever", None, &[], "/some/where/logs.txt");
    assert!(
        out.contains("Full logs: /some/where/logs.txt"),
        "got: {}",
        out
    );
}
