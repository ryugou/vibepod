use anyhow::Result;
use clap::Parser;
use vibepod::cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    // "vp" as alias works identically — no special handling needed,
    // clap already parses args regardless of binary name.
    let cli = Cli::parse();

    match cli.command {
        Commands::Init {} => {
            vibepod::cli::init::execute().await?;
        }
        Commands::Login {} => {
            vibepod::cli::login::execute().await?;
        }
        Commands::Logout {} => {
            vibepod::cli::logout::execute()?;
        }
        Commands::Run {
            resume,
            prompt,
            no_network,
            env,
            env_file,
            bridge,
            notify_delay,
            slack_channel,
            llm_provider,
        } => {
            vibepod::cli::run::execute(resume, prompt, no_network, env, env_file, bridge, notify_delay, slack_channel, llm_provider).await?;
        }
        Commands::Restore {} => {
            vibepod::cli::restore::execute()?;
        }
    }

    Ok(())
}
