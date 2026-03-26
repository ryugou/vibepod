use std::fs;
use tempfile::TempDir;
use vibepod::bridge::logger::BridgeLogger;

#[test]
fn test_log_notified_event() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("test-session.jsonl");
    let mut logger = BridgeLogger::new(&log_path).unwrap();

    logger
        .log_notified("Do you want to proceed? (y/n)")
        .unwrap();

    let content = fs::read_to_string(&log_path).unwrap();
    let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(line["event"], "notified");
    assert_eq!(line["last_lines"], "Do you want to proceed? (y/n)");
    assert!(line["ts"].is_string());
}

#[test]
fn test_log_responded_event() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("test-session.jsonl");
    let mut logger = BridgeLogger::new(&log_path).unwrap();

    logger.log_responded("slack_button", "y\n", 35).unwrap();

    let content = fs::read_to_string(&log_path).unwrap();
    let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(line["event"], "responded");
    assert_eq!(line["source"], "slack_button");
    assert_eq!(line["stdin_sent"], "y\n");
    assert_eq!(line["response_time_seconds"], 35);
}

#[test]
fn test_log_multiple_events() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("test-session.jsonl");
    let mut logger = BridgeLogger::new(&log_path).unwrap();

    logger.log_notified("Proceed? (y/n)").unwrap();
    logger.log_responded("terminal", "y\n", 10).unwrap();

    let content = fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = content.trim().lines().collect();
    assert_eq!(lines.len(), 2);

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["event"], "notified");

    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["event"], "responded");
}

#[test]
fn test_log_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("test-session.jsonl");
    let mut logger = BridgeLogger::new(&log_path).unwrap();

    logger.log_notified("test").unwrap();

    let metadata = fs::metadata(&log_path).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn test_log_creates_parent_directories() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("nested").join("dir").join("test.jsonl");
    let mut logger = BridgeLogger::new(&log_path).unwrap();

    logger.log_notified("test").unwrap();

    assert!(log_path.exists());
}
