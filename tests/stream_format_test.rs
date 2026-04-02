use vibepod::runtime::{format_stream_event, StreamEvent};

#[test]
fn test_format_assistant_text() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ファイルを確認します"}]}}"#;
    match format_stream_event(line) {
        StreamEvent::Display(s) => {
            assert_eq!(s, "  │  [assistant] ファイルを確認します");
        }
        other => panic!("Expected Display, got {:?}", other),
    }
}

#[test]
fn test_format_tool_use() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
    match format_stream_event(line) {
        StreamEvent::Display(s) => {
            assert!(s.contains("[tool_use]"), "Expected [tool_use] in: {}", s);
            assert!(s.contains("Read"), "Expected tool name Read in: {}", s);
            assert!(s.contains("file_path"), "Expected file_path in: {}", s);
            assert!(s.contains("src/main.rs"), "Expected file path in: {}", s);
        }
        other => panic!("Expected Display, got {:?}", other),
    }
}

#[test]
fn test_format_tool_use_truncation() {
    let long_value = "a".repeat(100);
    let line = format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"Write","input":{{"file_path":"{}"}}}}]}}}}"#,
        long_value
    );
    match format_stream_event(&line) {
        StreamEvent::Display(s) => {
            assert!(s.contains("...\""), "Expected truncation with ...: {}", s);
            // The truncated value should be 77 chars + ... in quotes
            assert!(!s.contains(&long_value), "Should not contain full value");
        }
        other => panic!("Expected Display, got {:?}", other),
    }
}

#[test]
fn test_format_result() {
    let line = r#"{"type":"result","result":"完了しました"}"#;
    match format_stream_event(line) {
        StreamEvent::Result(s) => {
            assert_eq!(s, "完了しました");
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

#[test]
fn test_format_system_event_skipped() {
    let line = r#"{"type":"system","subtype":"init"}"#;
    assert!(matches!(format_stream_event(line), StreamEvent::Skip));
}

#[test]
fn test_format_rate_limit_allowed_skipped() {
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
    assert!(matches!(format_stream_event(line), StreamEvent::Skip));
}

#[test]
fn test_format_rate_limit_rejected() {
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":"2026-04-02T12:00:00Z","rateLimitType":"five_hour"}}"#;
    match format_stream_event(line) {
        StreamEvent::Display(s) => {
            assert!(
                s.contains("[rate_limit]"),
                "Expected [rate_limit] in: {}",
                s
            );
            assert!(s.contains("rejected"), "Expected rejected in: {}", s);
            assert!(
                s.contains("2026-04-02T12:00:00Z"),
                "Expected timestamp in: {}",
                s
            );
            assert!(
                s.contains("five_hour"),
                "Expected rate limit type in: {}",
                s
            );
        }
        other => panic!("Expected Display, got {:?}", other),
    }
}

#[test]
fn test_format_invalid_json() {
    let line = "not json at all";
    match format_stream_event(line) {
        StreamEvent::PassThrough(s) => {
            assert_eq!(s, "not json at all");
        }
        other => panic!("Expected PassThrough, got {:?}", other),
    }
}

#[test]
fn test_format_multiple_content_blocks() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me check"},{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
    match format_stream_event(line) {
        StreamEvent::Display(s) => {
            assert!(s.contains("[assistant] Let me check"));
            assert!(s.contains("[tool_use] Read"));
        }
        other => panic!("Expected Display, got {:?}", other),
    }
}
