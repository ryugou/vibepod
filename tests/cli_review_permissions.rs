//! `--mode review` は `--dangerously-skip-permissions` を claude に渡してはならない。
//! Impl モードの自律実行（非対話）は従来通りバイパスを付ける（承認者不在のため）。
//! 対話モードはどちらの場合もバイパスしない。
//!
//! Docker 無しでも走らせたいので、`build_claude_args` を純関数として
//! 直接ユニットテストする。

use vibepod::cli::run::prepare::build_claude_args;
use vibepod::cli::run::RunOptions;
use vibepod::cli::RunMode;

fn base_opts() -> RunOptions {
    RunOptions {
        resume: false,
        prompt: Some("x".to_string()),
        no_network: false,
        env_vars: vec![],
        env_file: None,
        lang: None,
        worktree: false,
        mount: vec![],
        new_container: false,
        template: None,
        mode: RunMode::Impl,
        update_policy: vibepod::update::UpdatePolicy::default(),
        model: None,
        no_auto_build: false,
        timeout: None,
        verbose: false,
    }
}

#[test]
fn review_mode_autonomous_does_not_bypass_permissions() {
    let mut opts = base_opts();
    opts.mode = RunMode::Review;
    let args = build_claude_args(&opts, false); // non-interactive
    assert!(
        !args.iter().any(|a| a == "--dangerously-skip-permissions"),
        "review mode must NOT bypass permissions in autonomous runs; got {:?}",
        args
    );
    assert!(
        args.contains(&"-p".to_string()),
        "expected -p; got {:?}",
        args
    );
}

#[test]
fn impl_mode_autonomous_bypasses_permissions() {
    let opts = base_opts(); // mode = Impl
    let args = build_claude_args(&opts, false);
    assert!(
        args.iter().any(|a| a == "--dangerously-skip-permissions"),
        "impl mode autonomous must bypass permissions; got {:?}",
        args
    );
}

#[test]
fn interactive_mode_never_bypasses_permissions() {
    let opts = base_opts();
    let args = build_claude_args(&opts, true); // interactive
    assert!(
        !args.iter().any(|a| a == "--dangerously-skip-permissions"),
        "interactive mode must never bypass permissions; got {:?}",
        args
    );
}

#[test]
fn interactive_review_mode_never_bypasses_permissions() {
    let mut opts = base_opts();
    opts.mode = RunMode::Review;
    let args = build_claude_args(&opts, true);
    assert!(
        !args.iter().any(|a| a == "--dangerously-skip-permissions"),
        "interactive review mode must never bypass permissions; got {:?}",
        args
    );
}

/// Every review bundle must pin `"enabledPlugins": {}` (present AND empty).
///
/// In review mode the container runs `--dangerously-skip-permissions`, so its
/// safety rests entirely on the bundle's `settings.json`. The host's
/// `~/.claude/plugins` is bind-mounted into the container, and a plugin can
/// contribute commands/hooks that sidestep the `permissions.deny` list. An
/// explicit empty `enabledPlugins` is what keeps those mounted-but-inert:
/// nothing is activated. If a bundle dropped the key (Claude Code would then
/// fall back to its own default) or set it non-empty, that guarantee would
/// silently break — so this fails on either.
#[test]
fn all_review_bundles_pin_enabled_plugins_to_empty() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // The full set of review bundles vibepod ships. Kept explicit (not a
    // directory scan) so that a newly added review bundle without the key
    // would need to be listed here, drawing attention to the invariant.
    let bundles = ["generic", "go", "java", "node", "python", "rust"];

    for bundle in bundles {
        let path = manifest_dir
            .join("templates-data")
            .join(bundle)
            .join("review")
            .join("settings.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let json: serde_json::Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));

        let enabled_plugins = json.get("enabledPlugins").unwrap_or_else(|| {
            panic!(
                "{} is missing the `enabledPlugins` key; review bundles must \
                 pin it to {{}} so no host plugin is activated under \
                 --dangerously-skip-permissions",
                path.display()
            )
        });
        let obj = enabled_plugins.as_object().unwrap_or_else(|| {
            panic!(
                "{}: `enabledPlugins` must be an object, got {}",
                path.display(),
                enabled_plugins
            )
        });
        assert!(
            obj.is_empty(),
            "{}: `enabledPlugins` must be empty ({{}}), but activates {:?}. \
             A review bundle must not enable any plugin.",
            path.display(),
            obj.keys().collect::<Vec<_>>()
        );
    }
}
