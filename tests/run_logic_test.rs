use vibepod::cli::run::{
    build_review_prompt, detect_languages, get_lang_install_cmd, parse_mount_arg,
    resolve_reviewers, validate_slack_channel_id,
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
fn test_review_prompt_copilot() {
    let reviewers = vec!["copilot".to_string()];
    let result = build_review_prompt("my prompt", &reviewers);
    assert!(result.starts_with("my prompt"));
    assert!(result.contains("レビューフロー"));
    assert!(result.contains("gh pr create"));
    assert!(result.contains("add-reviewer copilot"));
    // 1ラウンドのみ（re-review 未サポート）
    assert!(result.contains("1ラウンド"));
    // インラインコメント取得 API
    assert!(result.contains("pulls/{number}/comments"));
}

#[test]
fn test_review_prompt_codex() {
    let reviewers = vec!["codex".to_string()];
    let result = build_review_prompt("my prompt", &reviewers);
    assert!(result.starts_with("my prompt"));
    assert!(result.contains("codex review"));
    assert!(result.contains("dangerously-bypass-approvals-and-sandbox"));
    assert!(result.contains("gh pr create"));
    // Codex ループ
    assert!(result.contains("指摘がなくなるまで"));
}

#[test]
fn test_review_prompt_both() {
    let reviewers = vec!["codex".to_string(), "copilot".to_string()];
    let result = build_review_prompt("my prompt", &reviewers);
    assert!(result.contains("Codex Review"));
    assert!(result.contains("Copilot Review"));
    // PR 作成は1回だけ
    assert_eq!(result.matches("gh pr create").count(), 1);
}

#[test]
fn test_no_review_prompt_unchanged() {
    let result = build_review_prompt("my prompt", &[]);
    assert_eq!(result, "my prompt");
}

// --- resolve_reviewers ---

#[test]
fn test_resolve_reviewers_none() {
    let config = vec!["copilot".to_string()];
    let result = resolve_reviewers(&None, &config);
    assert!(result.is_empty());
}

#[test]
fn test_resolve_reviewers_explicit() {
    let config = vec!["copilot".to_string()];
    let result = resolve_reviewers(&Some("codex".to_string()), &config);
    assert_eq!(result, vec!["codex".to_string()]);
}

#[test]
fn test_resolve_reviewers_from_config() {
    let config = vec!["copilot".to_string()];
    let result = resolve_reviewers(&Some("".to_string()), &config);
    assert_eq!(result, vec!["copilot".to_string()]);
}

#[test]
fn test_resolve_reviewers_empty_config() {
    let config: Vec<String> = vec![];
    let result = resolve_reviewers(&Some("".to_string()), &config);
    assert!(result.is_empty());
}

#[test]
fn test_resolve_reviewers_unknown_explicit_filtered() {
    let config = vec!["copilot".to_string()];
    let result = resolve_reviewers(&Some("unknown_tool".to_string()), &config);
    assert!(result.is_empty());
}

#[test]
fn test_resolve_reviewers_unknown_in_config_filtered() {
    let config = vec!["copilot".to_string(), "unknown_tool".to_string()];
    let result = resolve_reviewers(&Some("".to_string()), &config);
    assert_eq!(result, vec!["copilot".to_string()]);
}

// --- parse_mount_arg ---

#[test]
fn test_parse_mount_arg_with_colon() {
    let result = parse_mount_arg("/host/spec.md:/workspace/spec.md").unwrap();
    assert_eq!(
        result,
        (
            "/host/spec.md".to_string(),
            "/workspace/spec.md".to_string()
        )
    );
}

#[test]
fn test_parse_mount_arg_without_colon() {
    let result = parse_mount_arg("/host/spec.md").unwrap();
    assert_eq!(
        result,
        ("/host/spec.md".to_string(), "/mnt/spec.md".to_string())
    );
}

#[test]
fn test_parse_mount_arg_directory_without_colon() {
    let result = parse_mount_arg("/some/path/mydir").unwrap();
    assert_eq!(
        result,
        ("/some/path/mydir".to_string(), "/mnt/mydir".to_string())
    );
}

#[test]
fn test_parse_mount_arg_custom_container_path() {
    let result = parse_mount_arg("/foo/bar.txt:/custom/path.txt").unwrap();
    assert_eq!(
        result,
        ("/foo/bar.txt".to_string(), "/custom/path.txt".to_string())
    );
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
fn test_valid_slack_private_channel_id() {
    assert!(validate_slack_channel_id("G01ABC2DEF3"));
}

#[test]
fn test_invalid_slack_channel_id_too_short() {
    assert!(!validate_slack_channel_id("C123"));
}
