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
            lang,
            worktree,
            review,
            mount,
            reuse,
        } => {
            vibepod::cli::run::execute(vibepod::cli::run::RunOptions {
                resume,
                prompt,
                no_network,
                env_vars: env,
                env_file,
                lang,
                worktree,
                review,
                mount,
                reuse,
            })
            .await?;
        }
        Commands::Ps {} => {
            vibepod::cli::ps::execute().await?;
        }
        Commands::Logs {
            container,
            follow,
            tail,
        } => {
            vibepod::cli::logs::execute(container, follow, tail).await?;
        }
        Commands::Restore {} => {
            vibepod::cli::restore::execute()?;
        }
        Commands::Rm { name, all } => {
            vibepod::cli::rm::execute(name, all).await?;
        }
    }

    Ok(())
}
