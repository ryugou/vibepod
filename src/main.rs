use anyhow::Result;
use clap::Parser;
use vibepod::cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    // "vp" as alias works identically — no special handling needed,
    // clap already parses args regardless of binary name.
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { rebuild } => {
            vibepod::cli::init::execute(rebuild).await?;
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
            mount,
            new,
            update,
            no_update,
            model,
            no_auto_build,
            timeout,
            verbose,
        } => {
            // clap enforces that --update and --no-update are mutually
            // exclusive, so at most one of these can be set.
            let update_policy = if no_update {
                vibepod::update::UpdatePolicy::Disabled
            } else if update {
                vibepod::update::UpdatePolicy::Force
            } else {
                vibepod::update::UpdatePolicy::Auto
            };
            vibepod::cli::run::execute(vibepod::cli::run::RunOptions {
                resume,
                prompt,
                no_network,
                env_vars: env,
                env_file,
                lang,
                worktree,
                mount,
                new_container: new,
                update_policy,
                model,
                no_auto_build,
                timeout,
                verbose,
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
        Commands::Stop { name, all } => {
            vibepod::cli::stop::execute(name, all).await?;
        }
    }

    Ok(())
}
