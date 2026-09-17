//! Command-line arguments and configuration loading for the Sv2 CPU Miner binary.

use clap::Parser;
use std::path::PathBuf;
use stratum_apps::config_helpers::load_config;
use sv2_cpu_miner::config::Sv2CpuMinerConfig;
use sv2_cpu_miner::error::Sv2CpuMinerError;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the TOML configuration file",
        default_value = "config.toml"
    )]
    pub config_path: PathBuf,
    #[arg(
        short = 'f',
        long = "log-file",
        help = "Path to the log file. If not set, logs will only be written to stdout."
    )]
    pub log_file: Option<PathBuf>,
}

/// Prefix for environment variables that override config file values, e.g.
/// `CPU_MINER__SERVER_ADDR`.
const ENV_PREFIX: &str = "CPU_MINER";

/// Comma-separated list fields of [`Sv2CpuMinerConfig`] (see `load_config`).
const LIST_KEYS: &[&str] = &[];

/// Externally tagged enum fields of [`Sv2CpuMinerConfig`] (see `load_config`).
const ENUM_KEYS: &[&str] = &[];

/// Process CLI args, if any.
pub fn process_cli_args() -> Result<Sv2CpuMinerConfig, Sv2CpuMinerError> {
    let args = Args::parse();

    // Env vars prefixed `CPU_MINER__` override values from the optional TOML file.
    let mut config: Sv2CpuMinerConfig =
        load_config(&args.config_path, ENV_PREFIX, LIST_KEYS, ENUM_KEYS)?;
    config.validate()?;

    // The CLI flag wins over a log_file set in the config.
    if let Some(log_file) = args.log_file {
        config.log_file = Some(log_file);
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_CONFIG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/config.toml");

    /// The shipped example sets `auth_pk`, so this also proves the string-typed fields
    /// (`SocketAddr`, `Secp256k1PublicKey`) survive the loader.
    #[test]
    fn example_config_loads() {
        let config: Sv2CpuMinerConfig =
            load_config(EXAMPLE_CONFIG, ENV_PREFIX, LIST_KEYS, ENUM_KEYS)
                .unwrap_or_else(|e| panic!("config.toml must load: {e}"));
        assert!(config.auth_pk.is_some());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn env_var_overrides_file_value() {
        // SAFETY: the tests in this module are the only writers of this variable.
        unsafe { std::env::set_var("CPU_MINER__N_STANDARD_CHANNELS", "7") };
        let config: Sv2CpuMinerConfig =
            load_config(EXAMPLE_CONFIG, ENV_PREFIX, LIST_KEYS, ENUM_KEYS)
                .unwrap_or_else(|e| panic!("{e}"));
        unsafe { std::env::remove_var("CPU_MINER__N_STANDARD_CHANNELS") };
        assert_eq!(config.n_standard_channels, 7);
    }
}
