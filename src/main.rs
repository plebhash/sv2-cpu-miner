mod client;
mod config;
mod error;
mod handler;
mod miner;

use crate::client::Sv2CpuMiner;
use crate::config::Sv2CpuMinerConfig;

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the TOML configuration file
    #[arg(short, long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments
    let args = Args::parse();

    // Load configuration from file
    let config = Sv2CpuMinerConfig::from_file(args.config).unwrap_or_else(|e| {
        eprintln!("Sv2 CPU Miner config error: {e}");
        std::process::exit(1);
    });

    // Create and start the client
    let mut client = Sv2CpuMiner::new(config).await;

    // Use tokio::select to wait for either client completion or Ctrl+C
    let exit_code = tokio::select! {
        result = client.start() => match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                tracing::error!("Client error: {e}");
                ExitCode::FAILURE
            }
        },
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down...");
            ExitCode::SUCCESS
        }
    };

    // Shutdown the client
    client.shutdown().await;
    exit_code
}
