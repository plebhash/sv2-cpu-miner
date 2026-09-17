//! The `cpu_miner_sv2` binary: loads the configuration, runs one `Sv2CpuMiner` and turns
//! its outcome into the exit status.

mod args;

use cpu_miner_sv2::client::Sv2CpuMiner;
use std::process::ExitCode;
use stratum_apps::config_helpers::logging::init_logging;

use crate::args::process_cli_args;

#[tokio::main]
async fn main() -> ExitCode {
    let config = process_cli_args().unwrap_or_else(|e| {
        eprintln!("Sv2 CPU Miner config error: {e}");
        std::process::exit(1);
    });

    init_logging(config.log_file.as_deref());

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
