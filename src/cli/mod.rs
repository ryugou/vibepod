pub mod init;
pub mod login;
pub mod logout;
pub mod logs;
pub mod ps;
pub mod restore;
pub mod rm;
pub mod run;
pub mod stop;
pub mod template;

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
        /// Language toolchain to install in container (rust, node, python, go, java)
        #[arg(long)]
        lang: Option<String>,
        /// Run in an isolated git worktree (for --prompt mode)
        #[arg(long)]
        worktree: bool,
        /// Mount host file/directory into container (read-only). Repeatable.
        /// Format: <host-path>:<container-path> or <host-path> (mounted to /mnt/<filename>)
        #[arg(long, num_args = 1)]
        mount: Vec<String>,
        /// Force create a new container (error if running, replace if stopped)
        #[arg(long)]
        new: bool,
        /// Mount a vibepod-managed template into /home/vibepod/.claude/ instead of
        /// the host's ~/.claude/. Template directories live under
        /// ~/.config/vibepod/templates/<name>/.
        #[arg(long)]
        template: Option<String>,
    },
    /// List running VibePod containers
    Ps {},
    /// Show logs of a VibePod container
    Logs {
        /// Container name (defaults to most recent)
        #[arg()]
        container: Option<String>,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show from the end
        #[arg(short = 'n', long, default_value = "100")]
        tail: String,
    },
    /// Restore workspace to a previous session state
    Restore {},
    /// Remove VibePod containers
    Rm {
        /// Container name (or use --all)
        name: Option<String>,
        /// Remove all VibePod containers
        #[arg(long)]
        all: bool,
    },
    /// Stop VibePod containers (without removing them)
    Stop {
        /// Container name (or use --all)
        name: Option<String>,
        /// Stop all VibePod containers
        #[arg(long)]
        all: bool,
    },
    /// Manage vibepod templates (list / set-default)
    Template {
        #[command(subcommand)]
        subcommand: TemplateSubcommand,
    },
}

#[derive(Subcommand)]
pub enum TemplateSubcommand {
    /// List available templates (embedded and user-added)
    List {},
    /// Set the default template used when `--prompt` is passed without `--template`
    SetDefault {
        /// Template name (must exist in `vibepod template list`)
        name: String,
    },
}
