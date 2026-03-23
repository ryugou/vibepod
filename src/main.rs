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
        Commands::Logout { all } => {
            vibepod::cli::logout::execute(all)?;
        }
        Commands::Run {
            resume,
            prompt,
            no_network,
            env,
            env_file,
            isolated,
            name,
        } => {
            vibepod::cli::run::execute(resume, prompt, no_network, env, env_file, isolated, name)
                .await?;
        }
        Commands::Restore {} => {
            vibepod::cli::restore::execute()?;
        }
    }

    Ok(())
}
