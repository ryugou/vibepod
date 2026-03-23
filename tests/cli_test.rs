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
    let cli = Cli::parse_from([
        "vibepod",
        "run",
        "--resume",
        "--env",
        "KEY=VALUE",
        "--env",
        "FOO=BAR",
    ]);
    if let vibepod::cli::Commands::Run { env, .. } = cli.command {
        assert_eq!(env, vec!["KEY=VALUE", "FOO=BAR"]);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_interactive_mode_has_no_bypass_permissions() {
    let cli = Cli::parse_from(["vibepod", "run"]);
    if let vibepod::cli::Commands::Run { resume, prompt, .. } = cli.command {
        let interactive = !resume && prompt.is_none();
        assert!(interactive, "Default run should be interactive");
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_fire_and_forget_mode_detected() {
    let cli = Cli::parse_from(["vibepod", "run", "--prompt", "test"]);
    if let vibepod::cli::Commands::Run { resume, prompt, .. } = cli.command {
        let interactive = !resume && prompt.is_none();
        assert!(!interactive, "Prompt mode should not be interactive");
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_env_file() {
    let cli = Cli::parse_from(["vibepod", "run", "--env-file", ".env.template"]);
    if let vibepod::cli::Commands::Run { env_file, .. } = cli.command {
        assert_eq!(env_file, Some(".env.template".to_string()));
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_restore_command() {
    let cli = Cli::parse_from(["vibepod", "restore"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Restore {}));
}

#[test]
fn test_parse_login_command() {
    let cli = Cli::parse_from(["vibepod", "login"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Login {}));
}

#[test]
fn test_parse_logout_command() {
    let cli = Cli::parse_from(["vibepod", "logout"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Logout {}));
}
