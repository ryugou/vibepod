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
    /// Authenticate for container use
    Login {},
    /// Remove authentication session
    Logout {
        /// Also remove all isolated container sessions
        #[arg(long)]
        all: bool,
    },
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
        /// Use isolated auth session for this container
        #[arg(long)]
        isolated: bool,
        /// Name for isolated session (default: vibepod-<project>-isolated)
        #[arg(long)]
        name: Option<String>,
    },
    /// Restore workspace to a previous session state
    Restore {},
}
