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

#[test]
fn test_parse_run_with_lang() {
    let cli = Cli::parse_from(["vibepod", "run", "--lang", "rust"]);
    if let vibepod::cli::Commands::Run { lang, .. } = cli.command {
        assert_eq!(lang, Some("rust".to_string()));
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_worktree() {
    let cli = Cli::parse_from(["vibepod", "run", "--prompt", "test", "--worktree"]);
    if let vibepod::cli::Commands::Run { worktree, .. } = cli.command {
        assert!(worktree);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_single_mount() {
    let cli = Cli::parse_from([
        "vibepod",
        "run",
        "--mount",
        "/host/spec.md:/workspace/spec.md",
    ]);
    if let vibepod::cli::Commands::Run { mount, .. } = cli.command {
        assert_eq!(mount, vec!["/host/spec.md:/workspace/spec.md"]);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_with_multiple_mounts() {
    let cli = Cli::parse_from(["vibepod", "run", "--mount", "/a:/b", "--mount", "/c:/d"]);
    if let vibepod::cli::Commands::Run { mount, .. } = cli.command {
        assert_eq!(mount, vec!["/a:/b", "/c:/d"]);
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_run_without_mount() {
    let cli = Cli::parse_from(["vibepod", "run"]);
    if let vibepod::cli::Commands::Run { mount, .. } = cli.command {
        assert!(mount.is_empty());
    } else {
        panic!("Expected Run command");
    }
}

#[test]
fn test_parse_ps_command() {
    let cli = Cli::parse_from(["vibepod", "ps"]);
    assert!(matches!(cli.command, vibepod::cli::Commands::Ps {}));
}

#[test]
fn test_parse_logs_command_no_args() {
    let cli = Cli::parse_from(["vibepod", "logs"]);
    if let vibepod::cli::Commands::Logs {
        container,
        follow,
        tail,
    } = cli.command
    {
        assert_eq!(container, None);
        assert!(!follow);
        assert_eq!(tail, "100");
    } else {
        panic!("Expected Logs command");
    }
}

#[test]
fn test_parse_logs_command_with_container() {
    let cli = Cli::parse_from(["vibepod", "logs", "vibepod-myproject-abc123"]);
    if let vibepod::cli::Commands::Logs { container, .. } = cli.command {
        assert_eq!(container, Some("vibepod-myproject-abc123".to_string()));
    } else {
        panic!("Expected Logs command");
    }
}

#[test]
fn test_parse_logs_command_with_follow() {
    let cli = Cli::parse_from(["vibepod", "logs", "--follow"]);
    if let vibepod::cli::Commands::Logs { follow, .. } = cli.command {
        assert!(follow);
    } else {
        panic!("Expected Logs command");
    }
}

#[test]
fn test_parse_logs_command_with_tail() {
    let cli = Cli::parse_from(["vibepod", "logs", "-n", "50"]);
    if let vibepod::cli::Commands::Logs { tail, .. } = cli.command {
        assert_eq!(tail, "50");
    } else {
        panic!("Expected Logs command");
    }
}
