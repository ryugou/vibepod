use vibepod::cli::run::{
    build_claude_config_mounts, detect_languages, get_lang_install_cmd, parse_mount_arg,
    plugins_data_mount_entries, plugins_mount_entries, prepare_plugins_data_mount,
    prepare_sanitized_settings_mount, sanitize_settings_json, validate_slack_channel_id,
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

#[test]
fn test_plugins_mount_entries_non_colliding_home_returns_two() {
    // 通常のホスト（HOME != /home/vibepod）では二重マウントの (1) と (2) の
    // コンテナ側パスが異なり、2 本のエントリが返る。
    let home = std::path::PathBuf::from("/Users/alice");
    let entries = plugins_mount_entries("/Users/alice/.claude/plugins", &home);
    assert_eq!(entries.len(), 2, "expected two entries, got {:?}", entries);
    assert_eq!(
        entries[0],
        (
            "/Users/alice/.claude/plugins".to_string(),
            "/home/vibepod/.claude/plugins".to_string(),
        )
    );
    assert_eq!(
        entries[1],
        (
            "/Users/alice/.claude/plugins".to_string(),
            "/Users/alice/.claude/plugins".to_string(),
        )
    );
}

#[test]
fn test_plugins_mount_entries_colliding_home_dedupes_to_one() {
    // Linux のユーザー名が `vibepod` で HOME が `/home/vibepod` の場合、
    // (1) と (2) のコンテナ側パスが一致するため 1 本だけ返す。
    // （docker run -v が同一マウント先を拒否するのを避けるガード）
    let home = std::path::PathBuf::from("/home/vibepod");
    let entries = plugins_mount_entries("/home/vibepod/.claude/plugins", &home);
    assert_eq!(
        entries.len(),
        1,
        "expected dedup to 1 entry, got {:?}",
        entries
    );
    assert_eq!(
        entries[0],
        (
            "/home/vibepod/.claude/plugins".to_string(),
            "/home/vibepod/.claude/plugins".to_string(),
        )
    );
}

#[test]
fn test_claude_config_mounts_includes_plugins_via_helper() {
    // `build_claude_config_mounts` が plugins ディレクトリを検出したら
    // `plugins_mount_entries` の結果をそのまま組み込むことを確認する。
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(claude_dir.join("plugins")).unwrap();

    let mounts = build_claude_config_mounts(dir.path());
    let plugin_entries: Vec<_> = mounts
        .iter()
        .filter(|(_, dst)| dst.ends_with("/.claude/plugins"))
        .collect();
    assert_eq!(
        plugin_entries.len(),
        2,
        "tempdir home should produce two plugin mounts, got {:?}",
        plugin_entries
    );
}

// --- plugins_data_mount_entries ---
//
// `~/.claude/plugins` は read-only でマウントされるが、その内側の `data/`
// サブディレクトリだけを per-container の書き込み可能ステージへ差し替える。
// `plugins_mount_entries` と完全に対称な実装であるべきなので、同じ観点
// （二重マウント / dedup）でテストする。

#[test]
fn test_plugins_data_mount_entries_non_colliding_home_returns_two() {
    let home = std::path::PathBuf::from("/Users/alice");
    let entries = plugins_data_mount_entries("/staged/plugins-data", &home);
    assert_eq!(entries.len(), 2, "expected two entries, got {:?}", entries);
    assert_eq!(
        entries[0],
        (
            "/staged/plugins-data".to_string(),
            "/home/vibepod/.claude/plugins/data".to_string(),
        )
    );
    assert_eq!(
        entries[1],
        (
            "/staged/plugins-data".to_string(),
            "/Users/alice/.claude/plugins/data".to_string(),
        )
    );
}

#[test]
fn test_plugins_data_mount_entries_colliding_home_dedupes_to_one() {
    // ホスト HOME が /home/vibepod のとき、(1)(2) のコンテナ側パスが一致する
    // ため 1 本だけ返す（docker run -v が同一マウント先を拒否するのを避ける）。
    let home = std::path::PathBuf::from("/home/vibepod");
    let entries = plugins_data_mount_entries("/staged/plugins-data", &home);
    assert_eq!(
        entries.len(),
        1,
        "expected dedup to 1 entry, got {:?}",
        entries
    );
    assert_eq!(
        entries[0],
        (
            "/staged/plugins-data".to_string(),
            "/home/vibepod/.claude/plugins/data".to_string(),
        )
    );
}

// --- prepare_plugins_data_mount ---
//
// ホストの plugins/data の内容はコピーしない（他プロジェクトの codex job
// 履歴等を持ち込まないため）。ステージは常に空で作られる。

#[test]
fn test_prepare_plugins_data_mount_returns_none_when_plugins_dir_missing() {
    let home_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    // ~/.claude/plugins が存在しない（plugins 自体をマウントしないケース）

    let result = prepare_plugins_data_mount(home_dir.path(), runtime_dir.path()).unwrap();

    assert!(
        result.is_none(),
        "should return None when ~/.claude/plugins is absent"
    );
}

#[test]
fn test_prepare_plugins_data_mount_creates_host_dir_and_empty_stage() {
    let home_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home_dir.path().join(".claude/plugins")).unwrap();

    let result = prepare_plugins_data_mount(home_dir.path(), runtime_dir.path()).unwrap();
    let stage = result.expect("should return Some(stage) when ~/.claude/plugins exists");

    let host_data_dir = home_dir.path().join(".claude/plugins/data");
    assert!(
        host_data_dir.is_dir(),
        "host plugins/data mountpoint should be created (docker requires a real host dir \
         to bind-mount into)"
    );

    let stage_path = std::path::PathBuf::from(&stage);
    assert!(
        stage_path.is_dir(),
        "returned stage path must be a directory"
    );
    assert_eq!(stage_path, runtime_dir.path().join("plugins-data"));
    assert_eq!(
        std::fs::read_dir(&stage_path).unwrap().count(),
        0,
        "stage must be created empty (host content is not copied)"
    );
}

#[test]
#[cfg(unix)]
fn test_prepare_plugins_data_mount_sets_stage_permissions_to_0700() {
    // codex plugin のジョブ状態（プロンプト・レビュー出力＝ソースコード断片を
    // 含みうる）が書き込まれる領域のため、他ユーザーから読めないよう
    // 0700 を強制する（`sync_codex_entries_into` の codex ステージと同じ
    // パターン）。
    use std::os::unix::fs::PermissionsExt;

    let home_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home_dir.path().join(".claude/plugins")).unwrap();

    let result = prepare_plugins_data_mount(home_dir.path(), runtime_dir.path()).unwrap();
    let stage = result.expect("should return Some(stage) when ~/.claude/plugins exists");
    let stage_path = std::path::PathBuf::from(&stage);

    let mode = std::fs::metadata(&stage_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "plugins/data stage must be 0700 (found {:o})",
        mode
    );
}

#[test]
fn test_prepare_plugins_data_mount_does_not_copy_existing_host_files() {
    let home_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    let host_data_dir = home_dir.path().join(".claude/plugins/data");
    std::fs::create_dir_all(host_data_dir.join("codex-openai-codex/state")).unwrap();
    std::fs::write(
        host_data_dir.join("codex-openai-codex/state/other-project-job.json"),
        "SECRET_OTHER_PROJECT_DATA",
    )
    .unwrap();

    let result = prepare_plugins_data_mount(home_dir.path(), runtime_dir.path()).unwrap();
    let stage = result.expect("should return Some(stage)");
    let stage_path = std::path::PathBuf::from(&stage);

    assert_eq!(
        std::fs::read_dir(&stage_path).unwrap().count(),
        0,
        "stage must not copy existing host plugins/data content (e.g. other projects' codex \
         job history)"
    );
}

// --- prepare_plugins_data_mount: 既存ステージの扱い ---
//
// この関数はステージの存在と権限を保証するだけで、空にする責務は持たない
// （空にする責務は `reset_plugins_data_stage`、`src/cli/run/mod.rs` の
// `mod tests` を参照）。

#[test]
fn test_prepare_plugins_data_mount_does_not_clear_existing_content() {
    // この関数は既存コンテナの再利用・新規作成のどちらの経路からも呼ばれる
    // ため、それ自体は「新規作成か」を判断できない。既存ステージの内容は
    // 常に保持する（消すかどうかは呼び出し元が呼ぶ `reset_plugins_data_stage`
    // の責務）。
    let home_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home_dir.path().join(".claude/plugins")).unwrap();

    let stage_dir = runtime_dir.path().join("plugins-data");
    std::fs::create_dir_all(&stage_dir).unwrap();
    std::fs::write(stage_dir.join("in-progress-job.json"), "kept").unwrap();

    let result = prepare_plugins_data_mount(home_dir.path(), runtime_dir.path()).unwrap();
    let stage = result.expect("should return Some(stage)");
    let stage_path = std::path::PathBuf::from(&stage);

    assert!(
        stage_path.join("in-progress-job.json").is_file(),
        "prepare_plugins_data_mount must preserve existing stage content"
    );
}

#[test]
#[cfg(unix)]
fn test_prepare_plugins_data_mount_still_corrects_permissions() {
    // 既存ステージを保持する経路でも、パーミッションは削除ではなく
    // `set_permissions` で毎回 0700 に矯正される。既存ステージが何らかの
    // 理由で 0755 になっていても、内容は残したまま権限だけを直す。
    use std::os::unix::fs::PermissionsExt;

    let home_dir = tempfile::tempdir().unwrap();
    let runtime_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home_dir.path().join(".claude/plugins")).unwrap();

    let stage_dir = runtime_dir.path().join("plugins-data");
    std::fs::create_dir_all(&stage_dir).unwrap();
    std::fs::write(stage_dir.join("in-progress-job.json"), "kept").unwrap();
    std::fs::set_permissions(&stage_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    let result = prepare_plugins_data_mount(home_dir.path(), runtime_dir.path()).unwrap();
    let stage = result.expect("should return Some(stage)");
    let stage_path = std::path::PathBuf::from(&stage);

    let mode = std::fs::metadata(&stage_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "existing 0755 stage must be corrected to 0700, found {:o}",
        mode
    );
    assert!(
        stage_path.join("in-progress-job.json").is_file(),
        "permission correction must not delete existing content"
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

// --- sanitize_settings_json ---

#[test]
fn test_sanitize_settings_strips_hooks() {
    let input = r#"{
        "env": {"FOO": "bar"},
        "permissions": {"allow": ["Bash(ls:*)"]},
        "hooks": {
            "Notification": [
                {"matcher": "", "hooks": [{"type": "command", "command": "/Users/x/.claude/hooks/n.sh"}]}
            ]
        },
        "enabledPlugins": {"codex@openai-codex": true}
    }"#;

    let sanitized = sanitize_settings_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();

    assert!(parsed.get("hooks").is_none(), "hooks should be stripped");
    assert!(parsed.get("env").is_some(), "env should be preserved");
    assert!(
        parsed.get("permissions").is_some(),
        "permissions should be preserved"
    );
    assert!(
        parsed.get("enabledPlugins").is_some(),
        "enabledPlugins should be preserved"
    );
    assert_eq!(
        parsed["enabledPlugins"]["codex@openai-codex"],
        serde_json::Value::Bool(true)
    );
}

#[test]
fn test_sanitize_settings_strips_status_line() {
    let input = r#"{
        "env": {},
        "statusLine": {"type": "command", "command": "/Users/x/.claude/bin/status.sh"}
    }"#;

    let sanitized = sanitize_settings_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();

    assert!(
        parsed.get("statusLine").is_none(),
        "statusLine should be stripped"
    );
    assert!(parsed.get("env").is_some(), "env should be preserved");
}

#[test]
fn test_sanitize_settings_preserves_unknown_fields() {
    let input = r#"{
        "env": {"X": "1"},
        "teammateMode": "tmux",
        "extraKnownMarketplaces": {"foo": {"source": {"source": "github", "repo": "a/b"}}}
    }"#;

    let sanitized = sanitize_settings_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();

    assert_eq!(
        parsed["teammateMode"],
        serde_json::Value::String("tmux".to_string())
    );
    assert!(parsed.get("extraKnownMarketplaces").is_some());
}

#[test]
fn test_sanitize_settings_empty_object() {
    let sanitized = sanitize_settings_json("{}").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed.as_object().unwrap().len(), 0);
}

#[test]
fn test_sanitize_settings_invalid_json_errors() {
    let result = sanitize_settings_json("not valid json {");
    assert!(result.is_err(), "invalid JSON should return an error");
}

// --- prepare_sanitized_settings_mount ---

#[test]
fn test_prepare_sanitized_settings_mount_writes_and_returns_entry() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    // Create a fake ~/.claude/settings.json with hooks to be stripped
    let claude_dir = home_dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"env":{"X":"1"},"hooks":{"Notification":[]}}"#,
    )
    .unwrap();

    let result =
        prepare_sanitized_settings_mount(home_dir.path(), config_dir.path(), "vibepod-test-abc123")
            .unwrap();

    let (host_path, container_path) = result.expect("should return a mount entry");

    assert_eq!(container_path, "/home/vibepod/.claude/settings.json");
    assert!(
        host_path.contains("vibepod-test-abc123"),
        "host path should include container name: {}",
        host_path
    );

    // Verify the file was written and is sanitized
    let written = std::fs::read_to_string(&host_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert!(
        parsed.get("hooks").is_none(),
        "hooks should be stripped in written file"
    );
    assert!(parsed.get("env").is_some(), "env should be preserved");

    // Unix: 所有者のみ読み書き可能（0o600）に制限されていることを検証する
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&host_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "sanitized settings.json should have 0600 permissions, got {:o}",
            mode
        );
    }
}

#[test]
fn test_prepare_sanitized_settings_mount_no_host_settings() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    // No .claude/settings.json on host

    let result =
        prepare_sanitized_settings_mount(home_dir.path(), config_dir.path(), "vibepod-test-none")
            .unwrap();

    assert!(
        result.is_none(),
        "should return None when host settings.json is absent"
    );
}
