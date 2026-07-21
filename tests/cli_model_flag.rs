//! `--model <name>` は対話・非対話の両パスで `claude --model <name>` として
//! そのまま渡される。未指定時は `--model` を一切付けない。
//!
//! Docker 非依存で検証するため、純関数 `build_claude_args` を直接叩く。

use vibepod::cli::run::prepare::build_claude_args;
use vibepod::cli::run::RunOptions;

fn base_opts() -> RunOptions {
    RunOptions {
        resume: false,
        prompt: Some("do the thing".to_string()),
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

/// `--model X --model Y` の並びを検証するヘルパ: `--model` の直後の要素を返す。
fn model_value(args: &[String]) -> Option<&str> {
    let idx = args.iter().position(|a| a == "--model")?;
    args.get(idx + 1).map(|s| s.as_str())
}

#[test]
fn model_absent_adds_no_model_flag_noninteractive() {
    let opts = base_opts();
    let args = build_claude_args(&opts, false);
    assert!(
        !args.iter().any(|a| a == "--model"),
        "no --model expected when model is None; got {:?}",
        args
    );
}

#[test]
fn model_absent_adds_no_model_flag_interactive() {
    let opts = base_opts();
    let args = build_claude_args(&opts, true);
    assert!(
        !args.iter().any(|a| a == "--model"),
        "no --model expected when model is None; got {:?}",
        args
    );
}

#[test]
fn model_passed_through_noninteractive() {
    let mut opts = base_opts();
    opts.model = Some("claude-fable-5".to_string());
    let args = build_claude_args(&opts, false);
    assert_eq!(
        model_value(&args),
        Some("claude-fable-5"),
        "expected --model claude-fable-5; got {:?}",
        args
    );
}

#[test]
fn model_passed_through_interactive() {
    // interactive パス（prompt/resume なし）でも --model は渡る。
    let mut opts = base_opts();
    opts.prompt = None;
    opts.model = Some("some-model-id".to_string());
    let args = build_claude_args(&opts, true);
    assert_eq!(
        model_value(&args),
        Some("some-model-id"),
        "expected --model some-model-id in interactive args; got {:?}",
        args
    );
}

#[test]
fn model_value_is_not_validated() {
    // vibepod はモデル名を検証しない: 未知の文字列でもそのまま透過する。
    let mut opts = base_opts();
    opts.model = Some("totally-made-up-model".to_string());
    let args = build_claude_args(&opts, false);
    assert_eq!(model_value(&args), Some("totally-made-up-model"));
}
