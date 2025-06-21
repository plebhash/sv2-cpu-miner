mod client;
mod config;
mod handler;
mod miner;

use crate::client::Sv2CpuMiner;
use crate::config::Sv2CpuMinerConfig;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the TOML configuration file
    #[arg(short, long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments
    let args = Args::parse();

    // Load configuration from file
    let config = Sv2CpuMinerConfig::from_file(args.config)?;

    // Create and start the client
    let mut client = Sv2CpuMiner::new(config).await?;

    // Use tokio::select to wait for either client completion or Ctrl+C
    tokio::select! {
        result = client.start() => {
            if let Err(e) = result {
                tracing::error!("Client error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down...");
        }
    }

    // Shutdown the client
    client.shutdown().await;

    Ok(())
}
