//! Runtime configuration. The binary loads it from TOML with `CPU_MINER__*` environment
//! overrides; library users build it directly. The example `config.toml` documents each field.

use crate::error::Sv2CpuMinerError;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use stratum_apps::config_helpers::opt_path_from_toml;
use stratum_apps::key_utils::Secp256k1PublicKey;

/// One run's configuration; see the example `config.toml` for the meaning of each field.
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
    #[serde(default)]
    pub requires_standard_jobs: bool,
    #[serde(default, deserialize_with = "opt_path_from_toml")]
    pub log_file: Option<PathBuf>,
}

impl Sv2CpuMinerConfig {
    /// Checks the invariants the rest of the miner relies on, such as the Str0255 fields
    /// fitting their SV2 wire type.
    pub fn validate(&self) -> Result<(), Sv2CpuMinerError> {
        if self.nominal_hashrate_multiplier <= 0.0 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "nominal_hashrate_multiplier must be greater than 0.0",
            ));
        }

        if self.cpu_usage_percent == 0 || self.cpu_usage_percent > 100 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "cpu_usage_percent must be between 1 and 100",
            ));
        }

        if self.requires_standard_jobs && self.n_extended_channels > 0 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "n_extended_channels must be 0 when requires_standard_jobs is true",
            ));
        }

        if self.user_identity.len() > 255 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "user_identity must be at most 255 bytes",
            ));
        }

        if self.device_id.len() > 255 {
            return Err(Sv2CpuMinerError::InvalidConfig(
                "device_id must be at most 255 bytes",
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> Sv2CpuMinerConfig {
        Sv2CpuMinerConfig {
            server_addr: "127.0.0.1:3333".parse().unwrap(),
            auth_pk: None,
            n_extended_channels: 1,
            n_standard_channels: 1,
            user_identity: "user".to_string(),
            device_id: "cpu_miner_sv2".to_string(),
            single_submit: false,
            cpu_usage_percent: 100,
            nominal_hashrate_multiplier: 1.0,
            requires_standard_jobs: false,
            log_file: None,
        }
    }

    #[test]
    fn accepts_valid_config() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn rejects_user_identity_over_255_bytes() {
        let config = Sv2CpuMinerConfig {
            user_identity: "x".repeat(256),
            ..valid_config()
        };
        assert!(matches!(
            config.validate(),
            Err(Sv2CpuMinerError::InvalidConfig(msg)) if msg.contains("user_identity")
        ));
    }

    #[test]
    fn rejects_extended_channels_when_standard_jobs_are_required() {
        let config = Sv2CpuMinerConfig {
            requires_standard_jobs: true,
            n_extended_channels: 1,
            ..valid_config()
        };
        assert!(matches!(
            config.validate(),
            Err(Sv2CpuMinerError::InvalidConfig(msg)) if msg.contains("requires_standard_jobs")
        ));
    }
}
