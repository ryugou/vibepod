use vibepod::cli::run::{
    build_claude_config_mounts, detect_languages, get_lang_install_cmd, parse_mount_arg,
    validate_slack_channel_id,
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

// --- build_claude_config_mounts ---

#[test]
fn test_claude_config_mounts_constructed() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(claude_dir.join("skills")).unwrap();
    std::fs::create_dir_all(claude_dir.join("agents")).unwrap();
    std::fs::write(claude_dir.join("CLAUDE.md"), "# test").unwrap();

    let mounts = build_claude_config_mounts(dir.path());
    assert_eq!(mounts.len(), 3);

    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/CLAUDE.md"));
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/skills"));
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/agents"));
}

#[test]
fn test_claude_config_mounts_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let mounts = build_claude_config_mounts(dir.path());
    assert!(mounts.is_empty());
}

#[test]
fn test_claude_config_mounts_partial() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("CLAUDE.md"), "# test").unwrap();

    let mounts = build_claude_config_mounts(dir.path());
    assert_eq!(mounts.len(), 1);
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/CLAUDE.md"));
}

#[test]
fn test_claude_config_mounts_includes_plugins_at_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(claude_dir.join("plugins")).unwrap();

    let mounts = build_claude_config_mounts(dir.path());

    let plugins_host = claude_dir.join("plugins").to_string_lossy().to_string();
    let host_home_str = dir.path().to_string_lossy().to_string();
    let absolute_container_path = format!("{}/.claude/plugins", host_home_str);

    // Mount at /home/vibepod/.claude/plugins (where $HOME/.claude/plugins is read)
    assert!(
        mounts
            .iter()
            .any(|(src, dst)| src == &plugins_host && dst == "/home/vibepod/.claude/plugins"),
        "expected plugins mounted at /home/vibepod/.claude/plugins, got {:?}",
        mounts
    );

    // Mount at host-absolute path (where installed_plugins.json installPath points)
    assert!(
        mounts
            .iter()
            .any(|(src, dst)| src == &plugins_host && dst == &absolute_container_path),
        "expected plugins mounted at {}, got {:?}",
        absolute_container_path,
        mounts
    );
}

#[test]
fn test_claude_config_mounts_skips_plugins_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    // Intentionally no plugins/ directory

    let mounts = build_claude_config_mounts(dir.path());

    assert!(
        !mounts.iter().any(|(_, dst)| dst.ends_with("/plugins")),
        "expected no plugins mounts when ~/.claude/plugins is absent, got {:?}",
        mounts
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
