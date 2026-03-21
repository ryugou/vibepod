use anyhow::Result;
use clap::Parser;
use vibepod::cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
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
