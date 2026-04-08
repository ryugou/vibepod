use vibepod::cli::run::{
    build_claude_config_mounts, detect_languages, get_lang_install_cmd, parse_mount_arg,
    plugins_mount_entries, prepare_sanitized_settings_mount, sanitize_settings_json,
    template::{
        build_template_mounts, effective_template_name, embedded_template_names,
        extract_embedded_templates_if_missing, extract_single_embedded_template_if_missing,
        read_template_metadata, user_template_names,
    },
    validate_slack_channel_id, RunOptions,
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

// --- effective_template_name ---

fn make_run_options(template: Option<&str>, prompt: Option<&str>) -> RunOptions {
    RunOptions {
        resume: false,
        prompt: prompt.map(|s| s.to_string()),
        no_network: false,
        env_vars: Vec::new(),
        env_file: None,
        lang: None,
        worktree: false,
        mount: Vec::new(),
        new_container: false,
        template: template.map(|s| s.to_string()),
    }
}

fn empty_config() -> vibepod::config::VibepodConfig {
    vibepod::config::VibepodConfig::default()
}

/// Create a config dir (with no templates) and return its path-owning tempdir.
fn empty_config_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Build a (config, config_dir) pair where the global config has
/// `default_prompt_template = name` and a matching `templates/<name>/`
/// directory exists so the existence check in `effective_template_name`
/// passes.
fn config_with_default_template(name: &str) -> (vibepod::config::VibepodConfig, tempfile::TempDir) {
    let toml_content = format!("[run]\ndefault_prompt_template = \"{}\"\n", name);
    let dir = tempfile::tempdir().unwrap();
    let project_dir = dir.path().join("project");
    let global_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::write(global_dir.join("config.toml"), toml_content).unwrap();
    // Create the template dir so the existence check succeeds.
    std::fs::create_dir_all(global_dir.join("templates").join(name)).unwrap();
    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    (config, dir)
}

/// Same as above but **without** creating the template dir, used to
/// verify the host-mount fallback when the configured default is missing.
fn config_with_default_template_missing(
    name: &str,
) -> (vibepod::config::VibepodConfig, tempfile::TempDir) {
    let toml_content = format!("[run]\ndefault_prompt_template = \"{}\"\n", name);
    let dir = tempfile::tempdir().unwrap();
    let project_dir = dir.path().join("project");
    let global_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::write(global_dir.join("config.toml"), toml_content).unwrap();
    let config = vibepod::config::VibepodConfig::load(&project_dir, &global_dir).unwrap();
    (config, dir)
}

#[test]
fn test_effective_template_name_returns_opts_template_when_set() {
    let opts = make_run_options(Some("rust-code"), None);
    let cfg_dir = empty_config_dir();
    assert_eq!(
        effective_template_name(&opts, &empty_config(), cfg_dir.path()),
        Some("rust-code".to_string())
    );
}

#[test]
fn test_effective_template_name_returns_none_when_template_unset_interactive() {
    let opts = make_run_options(None, None);
    let cfg_dir = empty_config_dir();
    assert_eq!(
        effective_template_name(&opts, &empty_config(), cfg_dir.path()),
        None
    );
}

#[test]
fn test_effective_template_name_returns_none_when_prompt_no_default_config() {
    // --prompt あり、config に default_prompt_template なし → None
    let opts = make_run_options(None, Some("implement X"));
    let cfg_dir = empty_config_dir();
    assert_eq!(
        effective_template_name(&opts, &empty_config(), cfg_dir.path()),
        None
    );
}

#[test]
fn test_effective_template_name_returns_default_when_prompt_and_existing_template() {
    // --prompt あり、config に default あり、template も存在する → default を返す
    let opts = make_run_options(None, Some("implement X"));
    let (config, dir) = config_with_default_template("rust-code");
    assert_eq!(
        effective_template_name(&opts, &config, dir.path()),
        Some("rust-code".to_string())
    );
}

#[test]
fn test_effective_template_name_uses_user_dir_default_without_extract() {
    // ユーザーが自分で `templates/<name>/` を作って default に指定して
    // いる場合、embedded extraction の成否とは無関係にそのまま使えるべき。
    // (templates-data/ が空でも user-managed default は機能する)
    let opts = make_run_options(None, Some("implement X"));
    let (config, dir) = config_with_default_template("rust-code");
    // dir には既に `templates/rust-code/` がある (helper が作る)。
    assert_eq!(
        effective_template_name(&opts, &config, dir.path()),
        Some("rust-code".to_string())
    );
}

#[test]
fn test_effective_template_name_falls_back_when_default_template_missing() {
    // --prompt あり、config に default あり、しかし template が
    // ローカルにも embed にも存在しない → host mount フォールバック (None)。
    // これによって「default を設定しただけで run が壊れる」事故を防ぐ。
    let opts = make_run_options(None, Some("implement X"));
    let (config, dir) = config_with_default_template_missing("ghost-template");
    assert_eq!(effective_template_name(&opts, &config, dir.path()), None);
}

#[test]
fn test_effective_template_name_opts_template_overrides_default() {
    // opts.template が default を上書きする (存在チェックは行わない:
    // 明示指定はユーザー意図なので後段で fail-fast する)
    let opts = make_run_options(Some("review"), Some("implement X"));
    let (config, dir) = config_with_default_template("rust-code");
    assert_eq!(
        effective_template_name(&opts, &config, dir.path()),
        Some("review".to_string())
    );
}

#[test]
fn test_effective_template_name_interactive_ignores_default() {
    // interactive mode (prompt is None) では default template も無視して
    // host mount にフォールバック
    let opts = make_run_options(None, None);
    let (config, dir) = config_with_default_template("rust-code");
    assert_eq!(effective_template_name(&opts, &config, dir.path()), None);
}

#[test]
fn test_effective_template_name_worktree_ignores_default() {
    // --worktree + --prompt でも default template は適用しない。
    // worktree + template の併用は prepare_context で拒否されるため、
    // config による暗黙切替が worktree 実行を破壊しないよう guard する。
    let mut opts = make_run_options(None, Some("implement X"));
    opts.worktree = true;
    let (config, dir) = config_with_default_template("rust-code");
    assert_eq!(effective_template_name(&opts, &config, dir.path()), None);
}

#[test]
fn test_effective_template_name_worktree_still_honors_explicit_template() {
    // --worktree + 明示的 --template は effective_template_name としては
    // Some を返す (最終的な拒否は prepare_context の guard が行う)。
    // これにより拒否のエラーメッセージがユーザーに届く。
    let mut opts = make_run_options(Some("rust-code"), Some("implement X"));
    opts.worktree = true;
    let (config, dir) = config_with_default_template("review");
    assert_eq!(
        effective_template_name(&opts, &config, dir.path()),
        Some("rust-code".to_string())
    );
}

// --- build_template_mounts ---

#[test]
fn test_build_template_mounts_happy_path() {
    // plain-file plugins (no installed_plugins.json registry) は許可
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("my-template");
    std::fs::create_dir_all(template_dir.join("skills")).unwrap();
    std::fs::create_dir_all(template_dir.join("agents")).unwrap();
    std::fs::create_dir_all(template_dir.join("plugins")).unwrap();
    std::fs::write(template_dir.join("CLAUDE.md"), "# test").unwrap();
    std::fs::write(template_dir.join("settings.json"), "{}").unwrap();

    let mounts = build_template_mounts("my-template", config_dir.path()).unwrap();

    assert_eq!(mounts.len(), 5);
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/CLAUDE.md"));
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/skills"));
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/agents"));
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/plugins"));
    assert!(mounts
        .iter()
        .any(|(_, dst)| dst == "/home/vibepod/.claude/settings.json"));
}

#[test]
fn test_build_template_mounts_rejects_registry_missing_plugins_field() {
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("broken");
    std::fs::create_dir_all(template_dir.join("plugins")).unwrap();
    std::fs::write(
        template_dir.join("plugins").join("installed_plugins.json"),
        r#"{"version": 2}"#,
    )
    .unwrap();

    let err = build_template_mounts("broken", config_dir.path()).unwrap_err();
    assert!(
        err.to_string()
            .contains("missing a top-level 'plugins' object"),
        "expected shape-error about missing plugins field, got: {}",
        err
    );
}

#[test]
fn test_build_template_mounts_rejects_registry_entries_not_array() {
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("broken2");
    std::fs::create_dir_all(template_dir.join("plugins")).unwrap();
    std::fs::write(
        template_dir.join("plugins").join("installed_plugins.json"),
        r#"{"version": 2, "plugins": {"superpowers@claude-plugins-official": "not-an-array"}}"#,
    )
    .unwrap();

    let err = build_template_mounts("broken2", config_dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("must be an array"),
        "expected shape-error about non-array entries, got: {}",
        err
    );
}

#[test]
fn test_build_template_mounts_rejects_registry_entry_missing_installpath() {
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("broken3");
    std::fs::create_dir_all(template_dir.join("plugins")).unwrap();
    std::fs::write(
        template_dir.join("plugins").join("installed_plugins.json"),
        r#"{"version": 2, "plugins": {"superpowers@claude-plugins-official": [{"version": "5.0.7"}]}}"#,
    )
    .unwrap();

    let err = build_template_mounts("broken3", config_dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("missing a string 'installPath'"),
        "expected shape-error about missing installPath, got: {}",
        err
    );
}

#[test]
fn test_build_template_mounts_rejects_host_installpath_in_registry() {
    // user が host install を copy して作った template だと、
    // installed_plugins.json に host 絶対パス (/Users/alice/...) が
    // 残っていることがある。そのまま container に mount しても Claude
    // は plugin を解決できないので、vibepod CLI は明示的に bail する。
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("host-paths");
    std::fs::create_dir_all(template_dir.join("plugins").join("cache")).unwrap();
    std::fs::write(
        template_dir.join("plugins").join("installed_plugins.json"),
        r#"{
  "version": 2,
  "plugins": {
    "superpowers@claude-plugins-official": [
      {
        "scope": "user",
        "installPath": "/Users/alice/.claude/plugins/cache/claude-plugins-official/superpowers/5.0.7",
        "version": "5.0.7"
      }
    ]
  }
}"#,
    )
    .unwrap();

    let err = build_template_mounts("host-paths", config_dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("non-container installPath"),
        "expected error about non-container installPath, got: {}",
        msg
    );
    assert!(
        msg.contains("/home/vibepod/.claude/plugins/"),
        "expected error mentioning required container prefix, got: {}",
        msg
    );
}

#[test]
fn test_build_template_mounts_accepts_installed_plugins_json() {
    // Phase 4.5 以降、`plugins/installed_plugins.json` を含む template は
    // 「template が plugin を所有し container path を pre-bake している」
    // 前提で受け入れる。vibepod CLI は installPath の rewrite を行わず、
    // `plugins/` 全体を `/home/vibepod/.claude/plugins` に 1 点 bind する
    // だけ。template 作成者は installPath を container 絶対パス
    // (`/home/vibepod/.claude/plugins/cache/...`) で書く responsibility を持つ。
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("with-registry");
    // registry の installPath が指す実体を作る
    std::fs::create_dir_all(
        template_dir
            .join("plugins")
            .join("cache")
            .join("claude-plugins-official")
            .join("superpowers")
            .join("5.0.7"),
    )
    .unwrap();
    std::fs::write(
        template_dir.join("plugins").join("installed_plugins.json"),
        r#"{
  "version": 2,
  "plugins": {
    "superpowers@claude-plugins-official": [
      {
        "scope": "user",
        "installPath": "/home/vibepod/.claude/plugins/cache/claude-plugins-official/superpowers/5.0.7",
        "version": "5.0.7"
      }
    ]
  }
}"#,
    )
    .unwrap();

    let mounts = build_template_mounts("with-registry", config_dir.path()).unwrap();

    // plugins/ が /home/vibepod/.claude/plugins に 1 点 mount される
    assert!(
        mounts
            .iter()
            .any(|(_, dst)| dst == "/home/vibepod/.claude/plugins"),
        "expected plugins mount at /home/vibepod/.claude/plugins, got {:?}",
        mounts
    );
}

#[test]
fn test_build_template_mounts_rejects_registry_installpath_missing_on_disk() {
    // registry が指す installPath が container prefix で正しく始まって
    // いても、plugins_dir 配下に対応 cache dir が無い (stale な registry /
    // version 不一致 / typo) 場合は bail する。container 起動後の silent
    // な plugin 解決失敗を防ぐ。
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("stale-registry");
    std::fs::create_dir_all(template_dir.join("plugins")).unwrap();
    std::fs::write(
        template_dir.join("plugins").join("installed_plugins.json"),
        r#"{
  "version": 2,
  "plugins": {
    "superpowers@claude-plugins-official": [
      {
        "scope": "user",
        "installPath": "/home/vibepod/.claude/plugins/cache/claude-plugins-official/superpowers/9.9.9",
        "version": "9.9.9"
      }
    ]
  }
}"#,
    )
    .unwrap();

    let err = build_template_mounts("stale-registry", config_dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("no corresponding directory exists"),
        "expected error about missing plugin cache dir, got: {}",
        msg
    );
}

#[test]
fn test_build_template_mounts_rejects_registry_installpath_with_traversal() {
    // installPath に `..` を含む registry は path traversal として reject
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("traversal");
    std::fs::create_dir_all(template_dir.join("plugins")).unwrap();
    std::fs::write(
        template_dir.join("plugins").join("installed_plugins.json"),
        r#"{
  "version": 2,
  "plugins": {
    "superpowers@claude-plugins-official": [
      {
        "scope": "user",
        "installPath": "/home/vibepod/.claude/plugins/../../../etc/passwd",
        "version": "evil"
      }
    ]
  }
}"#,
    )
    .unwrap();

    let err = build_template_mounts("traversal", config_dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("path traversal"),
        "expected path traversal error, got: {}",
        err
    );
}

#[test]
fn test_build_template_mounts_partial_content() {
    // CLAUDE.md だけがある template
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("minimal");
    std::fs::create_dir_all(&template_dir).unwrap();
    std::fs::write(template_dir.join("CLAUDE.md"), "# minimal").unwrap();

    let mounts = build_template_mounts("minimal", config_dir.path()).unwrap();

    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].1, "/home/vibepod/.claude/CLAUDE.md");
}

#[test]
fn test_build_template_mounts_missing_template_errors() {
    let config_dir = tempfile::tempdir().unwrap();
    // template ディレクトリを作らない
    let err = build_template_mounts("nonexistent", config_dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Template 'nonexistent' not found"),
        "expected 'not found' error, got: {}",
        msg
    );
}

#[test]
fn test_build_template_mounts_empty_template_returns_zero_mounts() {
    // 空 template (ディレクトリだけあって中身 0 件) は「ホストの
    // ~/.claude を一切 mount しない = 素の Claude 環境で走らせる」
    // という opt-out パターン。エラーではなく空 Vec を返す。
    let config_dir = tempfile::tempdir().unwrap();
    let template_dir = config_dir.path().join("templates").join("blank");
    std::fs::create_dir_all(&template_dir).unwrap();

    let mounts = build_template_mounts("blank", config_dir.path()).unwrap();
    assert_eq!(mounts.len(), 0);
}

#[test]
fn test_build_template_mounts_rejects_path_traversal() {
    // `../` を含む template 名は path traversal の危険があるので
    // 拒否する（`~/.config/vibepod/templates/` の外に出るのを防ぐ）
    let config_dir = tempfile::tempdir().unwrap();
    let err = build_template_mounts("../etc", config_dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("invalid"),
        "expected 'invalid' error, got: {}",
        err
    );
}

#[test]
fn test_build_template_mounts_rejects_slash_in_name() {
    let config_dir = tempfile::tempdir().unwrap();
    let err = build_template_mounts("foo/bar", config_dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("invalid"),
        "expected 'invalid' error, got: {}",
        err
    );
}

#[test]
fn test_build_template_mounts_rejects_empty_name() {
    let config_dir = tempfile::tempdir().unwrap();
    let err = build_template_mounts("", config_dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("empty"),
        "expected 'empty' error, got: {}",
        err
    );
}

#[test]
fn test_build_template_mounts_accepts_valid_names() {
    // ASCII 英数字 / ハイフン / アンダースコアは OK。
    // ディレクトリが無いので not found エラーで確認する（validation は通る）
    let config_dir = tempfile::tempdir().unwrap();
    for name in &["rust-code", "my_template", "abc123", "a-b_c-1"] {
        let err = build_template_mounts(name, config_dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "valid name '{}' should pass validation but fail with 'not found', got: {}",
            name,
            msg
        );
    }
}

#[cfg(unix)]
#[test]
fn test_build_template_mounts_rejects_symlinked_entry_escape() {
    // template 内の settings.json が template root 外のファイルへの
    // symlink である場合、path traversal になるので reject する
    use std::os::unix::fs::symlink;

    let config_dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    let outside_file = outside_dir.path().join("secret.json");
    std::fs::write(&outside_file, r#"{"evil": true}"#).unwrap();

    let template_dir = config_dir.path().join("templates").join("malicious");
    std::fs::create_dir_all(&template_dir).unwrap();
    symlink(&outside_file, template_dir.join("settings.json")).unwrap();

    let err = build_template_mounts("malicious", config_dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("symlink escape") || msg.contains("outside"),
        "expected symlink escape error, got: {}",
        msg
    );
}

#[cfg(unix)]
#[test]
fn test_build_template_mounts_rejects_symlinked_template_dir_escape() {
    // template ディレクトリそのものが templates root 外への symlink
    // の場合も reject する
    use std::os::unix::fs::symlink;

    let config_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config_dir.path().join("templates")).unwrap();

    let outside_dir = tempfile::tempdir().unwrap();
    let evil_template = outside_dir.path().join("evil-template");
    std::fs::create_dir_all(&evil_template).unwrap();
    std::fs::write(evil_template.join("CLAUDE.md"), "# evil").unwrap();

    symlink(
        &evil_template,
        config_dir.path().join("templates").join("rogue"),
    )
    .unwrap();

    let err = build_template_mounts("rogue", config_dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("symlink escape") || msg.contains("outside"),
        "expected symlink escape error, got: {}",
        msg
    );
}

// --- Phase 3: template store + embed + enumeration ---

#[test]
fn test_user_template_names_empty_when_no_dir() {
    let config_dir = tempfile::tempdir().unwrap();
    let names = user_template_names(config_dir.path()).unwrap();
    assert!(names.is_empty());
}

#[test]
fn test_user_template_names_returns_subdirs_only() {
    let config_dir = tempfile::tempdir().unwrap();
    let templates = config_dir.path().join("templates");
    std::fs::create_dir_all(templates.join("alpha")).unwrap();
    std::fs::create_dir_all(templates.join("beta")).unwrap();
    // ファイルは無視される
    std::fs::write(templates.join("not_a_template.txt"), "").unwrap();

    let names = user_template_names(config_dir.path()).unwrap();
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
}

#[cfg(unix)]
#[test]
fn test_user_template_names_includes_in_root_symlinked_dir() {
    // templates/ 内の dir に張られた symlink (in-root を指す) は valid。
    // resolve_template_dir が通すので user_template_names も通すべき
    // (両者の集合一致が `template list` <-> `run --template` の整合性に
    // 必要)。
    let config_dir = tempfile::tempdir().unwrap();
    let templates = config_dir.path().join("templates");
    std::fs::create_dir_all(templates.join("real")).unwrap();
    std::os::unix::fs::symlink(templates.join("real"), templates.join("alias")).unwrap();

    let names = user_template_names(config_dir.path()).unwrap();
    assert_eq!(names, vec!["alias".to_string(), "real".to_string()]);
}

#[cfg(unix)]
#[test]
fn test_user_template_names_excludes_out_of_root_symlinked_dir() {
    // templates/ 外を指す symlink は escape として扱い、list から除外。
    // resolve_template_dir も reject するので runtime と整合する。
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(config_dir.join("templates")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, config_dir.join("templates").join("escape")).unwrap();

    let names = user_template_names(&config_dir).unwrap();
    assert!(
        names.is_empty(),
        "expected escape symlink to be filtered, got {:?}",
        names
    );
}

#[test]
fn test_user_template_names_filters_invalid_names() {
    let config_dir = tempfile::tempdir().unwrap();
    let templates = config_dir.path().join("templates");
    std::fs::create_dir_all(templates.join("valid")).unwrap();
    // 名前に `.` が入るものは validate_template_name で reject される
    std::fs::create_dir_all(templates.join("invalid.name")).unwrap();

    let names = user_template_names(config_dir.path()).unwrap();
    assert_eq!(names, vec!["valid".to_string()]);
}

#[cfg(unix)]
#[test]
fn test_user_template_names_propagates_unreadable_dir() {
    // templates/ が存在するが読み取り権限が無い場合、空配列ではなく
    // I/O エラーを伝播する。silent な空配列だと set-default が「該当
    // template が無い」と reject して原因不明になるため。
    use std::os::unix::fs::PermissionsExt;
    let config_dir = tempfile::tempdir().unwrap();
    let templates = config_dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    let mut perms = std::fs::metadata(&templates).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&templates, perms).unwrap();

    let result = user_template_names(config_dir.path());

    // restore so the tempdir cleanup can run
    let mut perms = std::fs::metadata(&templates).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&templates, perms).unwrap();

    // root user (CI) は permission を無視するので、その場合だけ skip。
    // 通常ユーザー実行ではエラーが返るはず。
    if let Ok(names) = &result {
        eprintln!(
            "running as root or perms ignored — got {:?}, skipping assertion",
            names
        );
        return;
    }
    assert!(result.is_err(), "expected I/O error, got {:?}", result);
}

#[test]
fn test_extract_embedded_templates_creates_official_templates() {
    // Phase 4 で templates-data/ に rust-code と review が追加されたので、
    // extract で `~/.config/vibepod/templates/rust-code/` と
    // `~/.config/vibepod/templates/review/` が作られることを確認する。
    let config_dir = tempfile::tempdir().unwrap();
    assert!(!config_dir.path().join("templates").exists());

    extract_embedded_templates_if_missing(config_dir.path()).unwrap();

    let templates = config_dir.path().join("templates");
    assert!(templates.is_dir(), "templates root should be created");
    assert!(
        templates.join("rust-code").is_dir(),
        "rust-code template should be extracted"
    );
    assert!(
        templates.join("review").is_dir(),
        "review template should be extracted"
    );
    // トップ階層に marker が書かれていること
    assert!(
        templates
            .join("rust-code")
            .join(".vibepod-embedded")
            .is_file(),
        "rust-code should have embedded marker"
    );
    assert!(
        templates.join("review").join(".vibepod-embedded").is_file(),
        "review should have embedded marker"
    );
    // CLAUDE.md が存在すること (template の中身が再帰的に展開されている)
    assert!(templates.join("rust-code").join("CLAUDE.md").is_file());
    assert!(templates.join("review").join("CLAUDE.md").is_file());

    // Phase 4.5: plugins/ も bundle されており、installed_plugins.json と
    // superpowers の cache が展開されている
    for template_name in ["rust-code", "review"] {
        let plugins = templates.join(template_name).join("plugins");
        assert!(
            plugins.is_dir(),
            "{} plugins/ should be extracted",
            template_name
        );
        assert!(
            plugins.join("installed_plugins.json").is_file(),
            "{} plugins/installed_plugins.json should be extracted",
            template_name
        );
        assert!(
            plugins
                .join("cache/claude-plugins-official/superpowers/5.0.7/CLAUDE.md")
                .is_file(),
            "{} should have superpowers plugin extracted",
            template_name
        );
    }
}

#[test]
fn test_extract_embedded_templates_is_idempotent() {
    let config_dir = tempfile::tempdir().unwrap();
    extract_embedded_templates_if_missing(config_dir.path()).unwrap();
    extract_embedded_templates_if_missing(config_dir.path()).unwrap();
    extract_embedded_templates_if_missing(config_dir.path()).unwrap();
    // 再呼び出しでもエラーにならず、結果が安定していること
    let templates = config_dir.path().join("templates");
    assert!(templates.join("rust-code").is_dir());
    assert!(templates.join("review").is_dir());
}

#[test]
fn test_extract_embedded_templates_preserves_existing_user_dir() {
    let config_dir = tempfile::tempdir().unwrap();
    // ユーザーが独自の (embedded に含まれない) template を作っている場合、
    // extract で公式 template が展開されても、ユーザー template は
    // 触られない。
    let user_template = config_dir.path().join("templates").join("my-custom");
    std::fs::create_dir_all(&user_template).unwrap();
    std::fs::write(user_template.join("CLAUDE.md"), "user content").unwrap();

    extract_embedded_templates_if_missing(config_dir.path()).unwrap();

    let content = std::fs::read_to_string(user_template.join("CLAUDE.md")).unwrap();
    assert_eq!(content, "user content");
    // 公式 template も同時に展開される
    assert!(config_dir
        .path()
        .join("templates")
        .join("rust-code")
        .is_dir());
}

#[test]
fn test_extract_embedded_templates_respects_user_override_same_name() {
    // ユーザーが embedded と同名の dir を自前で作っている場合、
    // extract はそれを尊重して上書きしない (marker 無しで残る)。
    let config_dir = tempfile::tempdir().unwrap();
    let templates = config_dir.path().join("templates");
    std::fs::create_dir_all(templates.join("rust-code")).unwrap();
    std::fs::write(
        templates.join("rust-code").join("CLAUDE.md"),
        "user override",
    )
    .unwrap();

    extract_embedded_templates_if_missing(config_dir.path()).unwrap();

    // 内容が維持される
    let content = std::fs::read_to_string(templates.join("rust-code").join("CLAUDE.md")).unwrap();
    assert_eq!(content, "user override");
    // marker は書かれない (user override として扱う)
    assert!(!templates
        .join("rust-code")
        .join(".vibepod-embedded")
        .is_file());
}

// --- read_template_metadata ---

#[test]
fn test_read_template_metadata_missing_file_returns_default() {
    // vibepod-template.toml が無い template は default (空 required_langs)
    let dir = tempfile::tempdir().unwrap();
    let meta = read_template_metadata(dir.path()).unwrap();
    assert!(meta.runtime.required_langs.is_empty());
}

#[test]
fn test_read_template_metadata_parses_required_langs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vibepod-template.toml"),
        r#"[runtime]
required_langs = ["rust", "python"]
"#,
    )
    .unwrap();
    let meta = read_template_metadata(dir.path()).unwrap();
    assert_eq!(
        meta.runtime.required_langs,
        vec!["rust".to_string(), "python".to_string()]
    );
}

#[test]
fn test_read_template_metadata_empty_runtime_section_ok() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vibepod-template.toml"), "[runtime]\n").unwrap();
    let meta = read_template_metadata(dir.path()).unwrap();
    assert!(meta.runtime.required_langs.is_empty());
}

#[test]
fn test_read_template_metadata_empty_file_ok() {
    // 完全に空の toml は default metadata 扱い
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vibepod-template.toml"), "").unwrap();
    let meta = read_template_metadata(dir.path()).unwrap();
    assert!(meta.runtime.required_langs.is_empty());
}

#[test]
fn test_read_template_metadata_rejects_invalid_toml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vibepod-template.toml"),
        "not valid [[[ toml",
    )
    .unwrap();
    let err = read_template_metadata(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("Failed to parse"),
        "expected parse error, got: {}",
        err
    );
}

#[test]
fn test_read_template_metadata_rejects_unknown_fields() {
    // deny_unknown_fields で future-proofing: 知らないキーは明示的に拒否
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vibepod-template.toml"),
        r#"[runtime]
required_langs = ["rust"]
something_new = "will be rejected until the field is added to the schema"
"#,
    )
    .unwrap();
    let err = read_template_metadata(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("Failed to parse"),
        "expected parse error on unknown field, got: {}",
        err
    );
}

#[test]
fn test_read_template_metadata_rejects_invalid_lang_name() {
    // path traversal / 空文字 / 制御文字などは validate で reject
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vibepod-template.toml"),
        r#"[runtime]
required_langs = ["../etc/passwd"]
"#,
    )
    .unwrap();
    let err = read_template_metadata(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("invalid required_langs entry"),
        "expected invalid lang error, got: {}",
        err
    );
}

#[test]
fn test_read_template_metadata_rejects_empty_lang_name() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vibepod-template.toml"),
        r#"[runtime]
required_langs = ["rust", ""]
"#,
    )
    .unwrap();
    let err = read_template_metadata(dir.path()).unwrap_err();
    assert!(err.to_string().contains("invalid required_langs entry"));
}

#[test]
fn test_read_template_metadata_rejects_unsupported_lang() {
    // "ruby" has no install command in get_lang_install_cmd, so letting
    // it through would silently drop the entry at setup time. Must bail.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vibepod-template.toml"),
        r#"[runtime]
required_langs = ["ruby"]
"#,
    )
    .unwrap();
    let err = read_template_metadata(dir.path()).unwrap_err();
    assert!(
        err.to_string()
            .contains("not a language vibepod knows how to install"),
        "expected unsupported-lang error, got: {}",
        err
    );
}

#[test]
fn test_read_template_metadata_rejects_lang_typo() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vibepod-template.toml"),
        r#"[runtime]
required_langs = ["rsut"]
"#,
    )
    .unwrap();
    let err = read_template_metadata(dir.path()).unwrap_err();
    assert!(
        err.to_string()
            .contains("not a language vibepod knows how to install"),
        "expected typo rejection, got: {}",
        err
    );
}

#[test]
fn test_embedded_rust_code_template_declares_rust_requirement() {
    // 公式 rust-code template の vibepod-template.toml が "rust" を要求する
    // こと。これが欠けると rust-code の quality gate が container で動かない
    // spec 違反に戻る。回帰防止の gate。
    let config_dir = tempfile::tempdir().unwrap();
    extract_single_embedded_template_if_missing(config_dir.path(), "rust-code").unwrap();
    let template_dir = config_dir.path().join("templates").join("rust-code");
    let meta = read_template_metadata(&template_dir).unwrap();
    assert_eq!(meta.runtime.required_langs, vec!["rust".to_string()]);
}

#[test]
fn test_embedded_review_template_declares_no_required_langs() {
    // review template は language-agnostic なので required_langs は空
    let config_dir = tempfile::tempdir().unwrap();
    extract_single_embedded_template_if_missing(config_dir.path(), "review").unwrap();
    let template_dir = config_dir.path().join("templates").join("review");
    let meta = read_template_metadata(&template_dir).unwrap();
    assert!(
        meta.runtime.required_langs.is_empty(),
        "expected review to have no required_langs, got: {:?}",
        meta.runtime.required_langs
    );
}

#[test]
fn test_embedded_template_names_contains_official_templates() {
    // Phase 4 で rust-code と review が embed される。
    let names = embedded_template_names();
    assert!(
        names.contains(&"rust-code".to_string()),
        "expected rust-code in embedded templates, got {:?}",
        names
    );
    assert!(
        names.contains(&"review".to_string()),
        "expected review in embedded templates, got {:?}",
        names
    );
    // 全て name validation を通る
    for name in &names {
        assert!(!name.is_empty());
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "embedded template name '{}' failed validation",
            name
        );
    }
}

#[test]
fn test_extract_single_embedded_template_installs_only_target() {
    // 単一ターゲット抽出は「要求された 1 つだけ」を展開し、他の embedded
    // template (rust-code / review) を巻き込まない。
    let config_dir = tempfile::tempdir().unwrap();
    extract_single_embedded_template_if_missing(config_dir.path(), "rust-code").unwrap();

    let templates = config_dir.path().join("templates");
    assert!(templates.join("rust-code").is_dir());
    assert!(templates
        .join("rust-code")
        .join(".vibepod-embedded")
        .is_file());
    // 他の template は展開されない (単一ターゲットモード)
    assert!(!templates.join("review").exists());
}

#[test]
fn test_extract_single_embedded_template_noop_for_unknown_name() {
    // embed 集合に存在しない名前は no-op (エラーにならず、templates dir
    // も変化しない)。呼び出し側 (prepare.rs) が existence check 後に
    // 呼ぶ場合の防御でもある。
    let config_dir = tempfile::tempdir().unwrap();
    extract_single_embedded_template_if_missing(config_dir.path(), "does-not-exist").unwrap();
    // ensure_templates_root だけは呼ばれるので templates/ は作られる
    // (あるいは作られないかもしれないが、少なくともこの呼び出しでは
    //  rust-code や review は生まれない)
    let templates = config_dir.path().join("templates");
    assert!(!templates.join("does-not-exist").exists());
    assert!(!templates.join("rust-code").exists());
    assert!(!templates.join("review").exists());
}

#[test]
fn test_extract_single_embedded_template_survives_sibling_conflict() {
    // 他の embedded 名が壊れた entry (regular file) として存在していても、
    // 要求された embedded template は正常に展開されることを確認する。
    // これが Phase 4 で単一-ターゲット API を導入した主な理由。
    let config_dir = tempfile::tempdir().unwrap();
    let templates = config_dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    // `review` を意図的にファイルとして置く → bulk extract だと他の
    // template まで止まるが、単一ターゲット (rust-code) は通るはず
    std::fs::write(templates.join("review"), "blocking file").unwrap();

    extract_single_embedded_template_if_missing(config_dir.path(), "rust-code").unwrap();

    assert!(templates.join("rust-code").is_dir());
    assert!(templates
        .join("rust-code")
        .join(".vibepod-embedded")
        .is_file());
    // 壊れた review はそのまま残る (単一ターゲット API は他を触らない)
    let content = std::fs::read_to_string(templates.join("review")).unwrap();
    assert_eq!(content, "blocking file");
}

#[test]
fn test_extract_single_embedded_template_noop_for_invalid_name() {
    // name validation を通らない文字列 (path traversal 攻撃想定) は
    // エラーにならず no-op。
    let config_dir = tempfile::tempdir().unwrap();
    extract_single_embedded_template_if_missing(config_dir.path(), "../evil").unwrap();
    extract_single_embedded_template_if_missing(config_dir.path(), "").unwrap();
    extract_single_embedded_template_if_missing(config_dir.path(), "has.dot").unwrap();
    // どの呼び出しでも templates dir に該当エントリは作られない
    let templates = config_dir.path().join("templates");
    if templates.exists() {
        for entry in std::fs::read_dir(&templates).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap_or_default();
            assert!(name != ".." && !name.contains("evil") && !name.contains("has.dot"));
        }
    }
}
