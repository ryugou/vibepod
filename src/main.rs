use anyhow::Result;
use clap::Parser;
use vibepod::cli::{Cli, Commands};

/// Check if invoked as "vp" alias — functionally identical to "vibepod"
fn get_binary_name() -> String {
    std::env::args()
        .next()
        .and_then(|arg| {
            std::path::Path::new(&arg)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "vibepod".to_string())
}

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
