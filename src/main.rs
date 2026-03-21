use anyhow::Result;
use clap::Parser;
use vibepod::cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    // "vp" as alias works identically — no special handling needed,
    // clap already parses args regardless of binary name.
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { claude_version } => {
            vibepod::cli::init::execute(claude_version).await?;
        }
        Commands::Run {
            resume,
            prompt,
            no_network,
            env,
        } => {
            vibepod::cli::run::execute(resume, prompt, no_network, env).await?;
        }
    }

    Ok(())
}
