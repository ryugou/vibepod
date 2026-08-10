//! `environment_preamble` は profile と workspace の状態から、コンテナ内
//! エージェントへ渡す環境情報ブロックを導出する純関数。
//! 設計: docs/superpowers/specs/2026-08-10-profile-visibility-and-environment-disclosure-design.md 3.3

use vibepod::cli::run::prepare::{build_claude_args, environment_preamble};
use vibepod::cli::run::RunOptions;

const PROMPT_DELIMITER: &str = "--- ここから利用者のプロンプト ---";

#[test]
fn swift_profile_returns_available_preamble() {
    let p =
        environment_preamble(Some("swift"), false).expect("swift profile must produce a preamble");
    assert!(p.contains("導入済み"), "got: {p}");
    assert!(
        p.ends_with(PROMPT_DELIMITER),
        "preamble must end with the delimiter; got: {p}"
    );
}

#[test]
fn swift_profile_ignores_package_swift_presence() {
    // 生成規則の表で profile = swift は Package.swift の有無を問わない。
    assert_eq!(
        environment_preamble(Some("swift"), true),
        environment_preamble(Some("swift"), false)
    );
}

#[test]
fn no_profile_with_package_swift_returns_absent_preamble() {
    let p = environment_preamble(None, true)
        .expect("Package.swift without profile must produce a preamble");
    assert!(p.contains("導入されていない"), "got: {p}");
    assert!(
        p.contains("profile = \"swift\""),
        "must point to the fix; got: {p}"
    );
    assert!(
        p.ends_with(PROMPT_DELIMITER),
        "preamble must end with the delimiter; got: {p}"
    );
}

#[test]
fn no_profile_without_package_swift_returns_none() {
    assert_eq!(environment_preamble(None, false), None);
}

#[test]
fn unknown_profile_returns_none() {
    // VALID_PROFILES へ swift 以外が追加されても、表に無い profile は None。
    assert_eq!(environment_preamble(Some("kotlin"), false), None);
}

fn base_opts() -> RunOptions {
    RunOptions {
        resume: false,
        prompt: Some("do the thing".to_string()),
        prompt_file: None,
        no_network: false,
        env_vars: vec![],
        env_file: None,
        lang: None,
        worktree: false,
        mount: vec![],
        new_container: false,
        update_policy: vibepod::update::UpdatePolicy::default(),
        model: None,
        no_auto_build: false,
        timeout: None,
        verbose: false,
    }
}

/// `-p` の直後の要素（claude へ渡るプロンプト本体）を返す。
fn prompt_value(args: &[String]) -> Option<&str> {
    let idx = args.iter().position(|a| a == "-p")?;
    args.get(idx + 1).map(|s| s.as_str())
}

#[test]
fn preamble_is_prepended_to_prompt() {
    let opts = base_opts();
    let args = build_claude_args(&opts, false, Some("ENV INFO"));
    assert_eq!(prompt_value(&args), Some("ENV INFO\ndo the thing"));
}

#[test]
fn prompt_is_unchanged_without_preamble() {
    let opts = base_opts();
    let args = build_claude_args(&opts, false, None);
    assert_eq!(prompt_value(&args), Some("do the thing"));
}

#[test]
fn preamble_does_not_appear_without_prompt() {
    // 対話モード / --resume は -p を使わないため前置経路を持たない。
    let mut opts = base_opts();
    opts.prompt = None;
    let args = build_claude_args(&opts, true, Some("ENV INFO"));
    assert!(!args.iter().any(|a| a == "-p"), "got: {args:?}");
    assert!(
        !args.iter().any(|a| a.contains("ENV INFO")),
        "got: {args:?}"
    );
}
