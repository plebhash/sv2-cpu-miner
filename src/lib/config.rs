use crate::error::Sv2CpuMinerError;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::Path;
use stratum_apps::config_helpers::load_config;
use stratum_apps::key_utils::Secp256k1PublicKey;

/// Duration of each CPU throttling cycle in milliseconds
/// The miner will work for N% of this window, then sleep for (100-N)% of this window
pub const CPU_THROTTLE_WINDOW_MS: u64 = 100;

/// Prefix for environment variables that override config file values, e.g.
/// `CPU_MINER__SERVER_ADDR`.
pub const ENV_PREFIX: &str = "CPU_MINER";

#[derive(Clone, Deserialize)]
pub struct Sv2CpuMinerConfig {
    pub server_addr: SocketAddr,
    pub auth_pk: Option<Secp256k1PublicKey>,
    pub n_extended_channels: u8,
    pub n_standard_channels: u8,
    pub user_identity: String,
    pub device_id: String,
    pub single_submit: bool,
    pub cpu_usage_percent: u64,
    pub nominal_hashrate_multiplier: f32,
}

impl Sv2CpuMinerConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Sv2CpuMinerError> {
        let config: Self = load_config(path, ENV_PREFIX, &[], &[])?;

        if config.nominal_hashrate_multiplier <= 0.0 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "nominal_hashrate_multiplier must be greater than 0.0",
            ));
        }

        if config.cpu_usage_percent == 0 || config.cpu_usage_percent > 100 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "cpu_usage_percent must be between 1 and 100",
            ));
        }

        if config.user_identity.len() > 255 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "user_identity must be at most 255 bytes",
            ));
        }

        if config.device_id.len() > 255 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "device_id must be at most 255 bytes",
            ));
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_temp_config(name: &str, user_identity: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("sv2-cpu-miner-{name}-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            format!(
                r#"
server_addr = "127.0.0.1:3333"
n_extended_channels = 1
n_standard_channels = 1
user_identity = "{user_identity}"
device_id = "sv2-cpu-miner"
single_submit = false
cpu_usage_percent = 100
nominal_hashrate_multiplier = 1.0
"#
            ),
        )
        .unwrap();
        path
    }

    /// The shipped example sets `auth_pk`, so this also proves the string-typed fields
    /// (`SocketAddr`, `Secp256k1PublicKey`) survive the loader.
    #[test]
    fn example_config_loads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config.toml");
        let config =
            Sv2CpuMinerConfig::load(path).unwrap_or_else(|e| panic!("config.toml must load: {e}"));
        assert!(config.auth_pk.is_some());
    }

    #[test]
    fn rejects_user_identity_over_255_bytes() {
        let path = write_temp_config("long-user-identity", &"x".repeat(256));
        let err = Sv2CpuMinerConfig::load(&path)
            .err()
            .expect("oversized user_identity must be rejected");
        let _ = std::fs::remove_file(&path);
        assert!(matches!(
            err,
            Sv2CpuMinerError::InvalidConfig(msg) if msg.contains("user_identity")
        ));
    }

    #[test]
    fn env_var_overrides_file_value() {
        // SAFETY: the tests in this module are the only writers of this variable.
        unsafe { std::env::set_var("CPU_MINER__N_STANDARD_CHANNELS", "7") };
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config.toml");
        let config = Sv2CpuMinerConfig::load(path).unwrap_or_else(|e| panic!("{e}"));
        unsafe { std::env::remove_var("CPU_MINER__N_STANDARD_CHANNELS") };
        assert_eq!(config.n_standard_channels, 7);
    }
}
