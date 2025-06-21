use key_utils::Secp256k1PublicKey;
use serde::Deserialize;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;

/// Duration of each CPU throttling cycle in milliseconds
/// The miner will work for N% of this window, then sleep for (100-N)% of this window
pub const CPU_THROTTLE_WINDOW_MS: u64 = 100;

#[derive(Deserialize)]
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
    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;

        if config.nominal_hashrate_multiplier <= 0.0 {
            anyhow::bail!("nominal_hashrate_multiplier must be greater than 0.0");
        }

        if config.cpu_usage_percent == 0 || config.cpu_usage_percent > 100 {
            anyhow::bail!("cpu_usage_percent must be between 1 and 100");
        }

        Ok(config)
    }
}
