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
