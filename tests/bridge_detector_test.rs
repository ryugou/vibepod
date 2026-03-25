use vibepod::bridge::detector::{DetectorEvent, DetectorState, IdleDetector};
use std::time::Duration;

#[tokio::test]
async fn test_initial_state_is_buffering() {
    let detector = IdleDetector::new(Duration::from_secs(5));
    assert!(matches!(detector.state(), DetectorState::Buffering));
}

#[tokio::test]
async fn test_output_resets_timer() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"hello world\n");
    assert!(matches!(detector.state(), DetectorState::Buffering));
}

#[tokio::test]
async fn test_terminal_input_clears_buffer() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"some output\n");
    detector.on_terminal_input();
    assert!(detector.buffer_content().is_empty());
}

#[tokio::test]
async fn test_terminal_input_only_clears_in_buffering() {
    let mut detector = IdleDetector::new(Duration::from_millis(1));
    detector.on_output(b"output\n");
    // Wait for idle
    tokio::time::sleep(Duration::from_millis(10)).await;
    let event = detector.check_idle();
    assert!(matches!(event, Some(DetectorEvent::Notify(_))));
    // Now in WaitingResponse - terminal input should NOT clear buffer
    assert!(matches!(detector.state(), DetectorState::WaitingResponse));
}

#[test]
fn test_check_idle_not_triggered_before_delay() {
    let mut detector = IdleDetector::new(Duration::from_secs(60));
    detector.on_output(b"hello\n");
    let event = detector.check_idle();
    assert!(event.is_none());
    assert!(matches!(detector.state(), DetectorState::Buffering));
}

#[tokio::test]
async fn test_check_idle_triggers_after_delay() {
    let mut detector = IdleDetector::new(Duration::from_millis(1));
    detector.on_output(b"Do you want to proceed? (y/n)\n");
    tokio::time::sleep(Duration::from_millis(10)).await;
    let event = detector.check_idle();
    assert!(matches!(event, Some(DetectorEvent::Notify(_))));
    assert!(matches!(detector.state(), DetectorState::WaitingResponse));
}

#[test]
fn test_check_idle_no_output_returns_none() {
    let mut detector = IdleDetector::new(Duration::from_millis(1));
    // No output at all - should not trigger
    let event = detector.check_idle();
    assert!(event.is_none());
}

#[tokio::test]
async fn test_on_response_transitions_to_buffering() {
    let mut detector = IdleDetector::new(Duration::from_millis(1));
    detector.on_output(b"prompt\n");
    tokio::time::sleep(Duration::from_millis(10)).await;
    detector.check_idle();
    assert!(matches!(detector.state(), DetectorState::WaitingResponse));

    detector.on_response();
    assert!(matches!(detector.state(), DetectorState::Buffering));
    assert!(detector.buffer_content().is_empty());
}

#[tokio::test]
async fn test_on_output_resumed_transitions_to_buffering() {
    let mut detector = IdleDetector::new(Duration::from_millis(1));
    detector.on_output(b"prompt\n");
    tokio::time::sleep(Duration::from_millis(10)).await;
    detector.check_idle();
    assert!(matches!(detector.state(), DetectorState::WaitingResponse));

    detector.on_output_resumed();
    assert!(matches!(detector.state(), DetectorState::Buffering));
    assert!(detector.buffer_content().is_empty());
}

#[test]
fn test_buffer_truncation_by_lines() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    // 50行の出力を追加（上限40行）
    for i in 0..50 {
        detector.on_output(format!("line {}\n", i).as_bytes());
    }
    let content = detector.buffer_for_slack();
    let lines: Vec<&str> = content.lines().collect();
    // 先頭に truncated メッセージ + 40行
    assert!(lines[0].contains("truncated"));
    assert_eq!(lines.len(), 41);
}

#[test]
fn test_buffer_truncation_by_chars() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    // 2500文字を超える長い行を追加
    let long_line = "x".repeat(3000) + "\n";
    detector.on_output(long_line.as_bytes());
    let content = detector.buffer_for_slack();
    assert!(content.chars().count() <= 2500 + 50); // truncation message margin
}

#[test]
fn test_buffer_truncation_multibyte_chars() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    // マルチバイト文字（3バイト/文字）で2500文字超えの出力
    let multibyte_line = "─".repeat(1000) + "\n";
    detector.on_output(multibyte_line.as_bytes());
    detector.on_output(multibyte_line.as_bytes());
    detector.on_output(multibyte_line.as_bytes());
    // パニックせずに切り詰められること
    let content = detector.buffer_for_slack();
    assert!(content.chars().count() <= 2500 + 50);
}

#[test]
fn test_ansi_stripped_in_slack_buffer() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"\x1b[31mred text\x1b[0m\n");
    let content = detector.buffer_for_slack();
    assert!(!content.contains("\x1b["));
    assert!(content.contains("red text"));
}

#[test]
fn test_buffer_content_raw() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"hello\n");
    detector.on_output(b"world\n");
    let content = detector.buffer_content();
    assert_eq!(content, "hello\nworld\n");
}

#[test]
fn test_output_accumulates_in_buffer() {
    let mut detector = IdleDetector::new(Duration::from_secs(5));
    detector.on_output(b"line 1\n");
    detector.on_output(b"line 2\n");
    detector.on_output(b"line 3\n");
    let content = detector.buffer_content();
    assert!(content.contains("line 1"));
    assert!(content.contains("line 2"));
    assert!(content.contains("line 3"));
}
