use clap::Parser;
use vibepod::cli::Cli;

#[test]
fn test_parse_init_command() {
    let cli = Cli::parse_from(["vibepod", "init"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Init { .. }));
}

#[test]
fn test_parse_run_with_resume() {
    let cli = Cli::parse_from(["vibepod", "run", "--resume"]);
    if let vibepod::cli::Commands::Run { resume, .. } = cli.command {
        assert!(resume);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_prompt() {
    let cli = Cli::parse_from(["vibepod", "run", "--prompt", "build the app"]);
    if let vibepod::cli::Commands::Run { prompt, .. } = cli.command {
        assert_eq!(prompt, Some("build the app".to_string()));
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_env() {
    let cli = Cli::parse_from(["vibepod", "run", "--resume", "--env", "KEY=VALUE", "--env", "FOO=BAR"]);
    if let vibepod::cli::Commands::Run { env, .. } = cli.command {
        assert_eq!(env, vec!["KEY=VALUE", "FOO=BAR"]);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_init_with_claude_version() {
    let cli = Cli::parse_from(["vibepod", "init", "--claude-version", "1.2.3"]);
    if let vibepod::cli::Commands::Init { claude_version, .. } = cli.command {
        assert_eq!(claude_version, Some("1.2.3".to_string()));
    } else {
        panic!("Expected Init command");
    }
}
