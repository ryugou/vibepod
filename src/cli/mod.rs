pub mod init;
pub mod login;
pub mod logout;
pub mod restore;
pub mod run;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "vibepod",
    about = "Safely run AI coding agents in Docker containers"
)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize VibePod (build Docker image)
    Init {},
    /// Authenticate for container use (creates long-lived token)
    Login {},
    /// Remove authentication token
    Logout {},
    /// Run AI agent in a container
    Run {
        /// Resume previous session
        #[arg(long)]
        resume: bool,
        /// Initial prompt for the agent (fire-and-forget mode)
        #[arg(long)]
        prompt: Option<String>,
        /// Disable network access in the container
        #[arg(long)]
        no_network: bool,
        /// Environment variables to pass (KEY=VALUE)
        #[arg(long, num_args = 1)]
        env: Vec<String>,
        /// Environment file (supports op:// references via 1Password CLI)
        #[arg(long)]
        env_file: Option<String>,
        /// Enable Slack Bridge mode for remote notifications
        #[arg(long)]
        bridge: bool,
        /// Idle detection delay in seconds before Slack notification (default: 30)
        #[arg(long, default_value = "30")]
        notify_delay: u64,
        /// Override Slack channel ID for notifications
        #[arg(long)]
        slack_channel: Option<String>,
        /// LLM provider for formatting notifications: anthropic (default), gemini, openai, none
        #[arg(long, default_value = "anthropic")]
        llm_provider: String,
        /// Language toolchain to install in container (rust, node, python, go, java)
        #[arg(long)]
        lang: Option<String>,
        /// Run in an isolated git worktree (for --prompt mode)
        #[arg(long)]
        worktree: bool,
        /// Auto-create PR and request GitHub Copilot review after implementation (requires --prompt)
        #[arg(long)]
        review: bool,
    },
    /// Restore workspace to a previous session state
    Restore {},
}
