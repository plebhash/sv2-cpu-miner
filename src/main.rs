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
use stratum_apps::config_helpers::logging::init_logging;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the TOML configuration file
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
    /// Path to the log file. If not set, logs will only be written to stdout.
    #[arg(short = 'f', long = "log-file")]
    log_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Parse command line arguments
    let args = Args::parse();

    // Load configuration from file, with CPU_MINER__* environment overrides
    let config = Sv2CpuMinerConfig::load(args.config).unwrap_or_else(|e| {
        eprintln!("Sv2 CPU Miner config error: {e}");
        std::process::exit(1);
    });

    init_logging(args.log_file.as_deref());

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
