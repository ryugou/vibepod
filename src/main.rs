use anyhow::Result;

fn main() -> Result<()> {
    println!("VibePod v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
