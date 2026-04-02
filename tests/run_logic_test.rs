use vibepod::cli::run::{
    build_review_prompt, detect_languages, get_lang_install_cmd, validate_slack_channel_id,
};

// --- detect_languages ---

#[test]
fn test_detect_rust() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
    let langs = detect_languages(dir.path());
    assert_eq!(langs, vec![("rust".to_string(), "Cargo.toml")]);
}

#[test]
fn test_detect_node() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    let langs = detect_languages(dir.path());
    assert_eq!(langs, vec![("node".to_string(), "package.json")]);
}

#[test]
fn test_detect_multiple_languages() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    let langs = detect_languages(dir.path());
    assert!(langs.iter().any(|(n, _)| n == "rust"));
    assert!(langs.iter().any(|(n, _)| n == "node"));
    assert_eq!(langs.len(), 2);
}

#[test]
fn test_detect_no_languages() {
    let dir = tempfile::tempdir().unwrap();
    let langs = detect_languages(dir.path());
    assert!(langs.is_empty());
}

// --- get_lang_install_cmd ---

#[test]
fn test_lang_install_cmd_rust() {
    let cmd = get_lang_install_cmd("rust");
    assert!(cmd.is_some());
    let cmd = cmd.unwrap();
    assert!(cmd.contains("rustup"));
    assert!(cmd.contains("build-essential"));
}

#[test]
fn test_lang_install_cmd_unknown() {
    let cmd = get_lang_install_cmd("unknown");
    assert!(cmd.is_none());
}

// --- build_review_prompt ---

#[test]
fn test_review_prompt_injection() {
    let result = build_review_prompt("my prompt", true);
    assert!(result.starts_with("my prompt"));
    assert!(result.contains("レビューフロー"));
    assert!(result.contains("git checkout -b"));
    assert!(result.contains("gh pr create"));
    assert!(result.contains("gh api repos/"));
}

#[test]
fn test_no_review_prompt_unchanged() {
    let result = build_review_prompt("my prompt", false);
    assert_eq!(result, "my prompt");
}

// --- validate_slack_channel_id ---

#[test]
fn test_valid_slack_channel_id() {
    assert!(validate_slack_channel_id("C01ABC2DEF3"));
}

#[test]
fn test_invalid_slack_channel_id_wrong_prefix() {
    assert!(!validate_slack_channel_id("U01ABC2DEF3"));
}

#[test]
fn test_invalid_slack_channel_id_too_short() {
    assert!(!validate_slack_channel_id("C123"));
}
