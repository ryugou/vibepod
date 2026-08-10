pub mod init;
pub mod login;
pub mod logout;
pub mod logs;
pub mod ps;
pub mod restore;
pub mod rm;
pub mod run;
pub mod stop;

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
    Init {
        /// Rebuild the image from scratch, ignoring the Docker layer cache and
        /// pulling a fresh base image. Use this to pick up a newer Claude Code
        /// than the cached `install.sh` layer contains.
        #[arg(long)]
        rebuild: bool,
    },
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
        /// Read the initial prompt from a file instead of the command line
        /// (fire-and-forget mode). The file content is passed through
        /// unmodified (not trimmed), bypassing host shell interpretation of
        /// special characters (`<`, `{`, backticks, `$`, ...). Mutually
        /// exclusive with `--prompt`.
        #[arg(long, conflicts_with = "prompt")]
        prompt_file: Option<String>,
        /// Disable network access in the container
        #[arg(long)]
        no_network: bool,
        /// Environment variables to pass (KEY=VALUE)
        #[arg(long, num_args = 1)]
        env: Vec<String>,
        /// Environment file (supports op:// references via 1Password CLI)
        #[arg(long)]
        env_file: Option<String>,
        /// Language toolchain to install in container (rust, node, python, go, java).
        /// Swift toolchain: set `profile = "swift"` in .vibepod/config.toml (see README)
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
        /// Check for a Claude Code update in the container now, ignoring the
        /// usual once-a-day throttle.
        #[arg(long, conflicts_with = "no_update")]
        update: bool,
        /// Skip the container's Claude Code update check entirely.
        #[arg(long)]
        no_update: bool,
        /// Model name passed straight through to `claude --model <name>`
        /// inside the container (e.g. a specific Claude model id). The value
        /// is not validated by vibepod — Claude Code decides if it is valid.
        /// Omit to let Claude Code resolve its own default.
        #[arg(long)]
        model: Option<String>,
        /// Do not automatically build the Docker image when it is missing.
        /// By default `vibepod run` builds the image on demand; pass this to
        /// fail fast with an instruction to run `vibepod init` instead.
        #[arg(long)]
        no_auto_build: bool,
        /// Wall-clock limit for a `--prompt` session. Accepts bare seconds
        /// (e.g. `1800`) or a duration (`30m`, `1h30m`). `0` disables the
        /// limit. Defaults to 30 minutes when omitted. On timeout the
        /// container-side agent is stopped and the run exits non-zero;
        /// workspace changes made by the agent (commits and uncommitted
        /// edits alike) are left in place, not reset. `vibepod restore`
        /// only works with a clean tree; see the README for recovery
        /// steps when uncommitted changes remain or `--worktree` was used.
        #[arg(long)]
        timeout: Option<String>,
        /// Stream Claude Code's per-event activity to stdout during a
        /// `--prompt` run (the pre-1.7 behavior). By default only a concise
        /// end-of-run summary is printed; the full stream is always saved to
        /// the session `logs.txt` regardless of this flag.
        #[arg(long)]
        verbose: bool,
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
}
