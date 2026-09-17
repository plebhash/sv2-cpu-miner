//! Common-protocol messages from the mining server. Only the `SetupConnection` reply is
//! expected; it is validated here, and anything else is a protocol violation.

use super::Sv2CpuMiner;
use crate::error::Sv2CpuMinerError;
use stratum_apps::stratum_core::common_messages_sv2::{
    ChannelEndpointChangedOwned, MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED, MESSAGE_TYPE_RECONNECT,
    ReconnectOwned, SetupConnectionErrorOwned, SetupConnectionSuccessOwned,
};
use stratum_apps::stratum_core::handlers_sv2::{
    HandleCommonMessagesFromServerOwnedAsync, HandlerErrorType,
};
use stratum_apps::stratum_core::parsers_sv2::Tlv;
use stratum_apps::utils::types::SUPPORTED_PROTOCOL_VERSION;
use tracing::{error, info};

/// `SetupConnection.Success` flag bits a mining server may set (server flags table of the
/// Mining Protocol spec). Stratum defines no names for them yet.
const REQUIRES_FIXED_VERSION: u32 = 0b01;
const REQUIRES_EXTENDED_CHANNELS: u32 = 0b10;

impl HandleCommonMessagesFromServerOwnedAsync for Sv2CpuMiner {
    type Error = Sv2CpuMinerError;

    fn get_negotiated_extensions_with_server(
        &self,
        _server_id: Option<usize>,
    ) -> Result<Vec<u16>, Self::Error> {
        Ok(vec![])
    }

    async fn handle_setup_connection_success(
        &mut self,
        _server_id: Option<usize>,
        msg: SetupConnectionSuccessOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received SetupConnection.Success: {}", msg);

        if msg.used_version != SUPPORTED_PROTOCOL_VERSION {
            return Err(Sv2CpuMinerError::SetupConnectionMismatch(format!(
                "used_version {} is not the offered version {}",
                msg.used_version, SUPPORTED_PROTOCOL_VERSION
            )));
        }

        let unknown_flags = msg.flags & !(REQUIRES_FIXED_VERSION | REQUIRES_EXTENDED_CHANNELS);
        if unknown_flags != 0 {
            return Err(Sv2CpuMinerError::SetupConnectionMismatch(format!(
                "flags 0x{unknown_flags:08x} are not defined for the Mining Protocol"
            )));
        }

        if msg.flags & REQUIRES_EXTENDED_CHANNELS != 0 && self.config.n_standard_channels > 0 {
            return Err(Sv2CpuMinerError::SetupConnectionMismatch(format!(
                "the mining server requires extended channels, but n_standard_channels is {}",
                self.config.n_standard_channels
            )));
        }

        // REQUIRES_FIXED_VERSION needs no action: this miner never rolls version bits.
        Ok(())
    }

    async fn handle_setup_connection_error(
        &mut self,
        _server_id: Option<usize>,
        msg: SetupConnectionErrorOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received SetupConnection.Error: {}", msg);
        Err(Sv2CpuMinerError::SetupConnectionFailed)
    }

    async fn handle_channel_endpoint_changed(
        &mut self,
        _server_id: Option<usize>,
        _msg: ChannelEndpointChangedOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received unexpected ChannelEndpointChanged");
        Err(Sv2CpuMinerError::unexpected_message(
            0,
            MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED,
        ))
    }

    async fn handle_reconnect(
        &mut self,
        _server_id: Option<usize>,
        _msg: ReconnectOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received unexpected Reconnect");
        Err(Sv2CpuMinerError::unexpected_message(
            0,
            MESSAGE_TYPE_RECONNECT,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Sv2CpuMinerConfig;
    use tokio_util::sync::CancellationToken;

    fn miner(n_standard_channels: u8) -> Sv2CpuMiner {
        Sv2CpuMiner {
            config: Sv2CpuMinerConfig {
                server_addr: "127.0.0.1:3333".parse().unwrap(),
                auth_pk: None,
                n_extended_channels: 0,
                n_standard_channels,
                user_identity: "user".to_string(),
                device_id: "sv2-cpu-miner".to_string(),
                single_submit: false,
                cpu_usage_percent: 100,
                nominal_hashrate_multiplier: 1.0,
                requires_standard_jobs: false,
                log_file: None,
            },
            nominal_hashrate: 1000.0,
            cancellation_token: CancellationToken::new(),
        }
    }

    async fn setup_success(
        miner: &mut Sv2CpuMiner,
        used_version: u16,
        flags: u32,
    ) -> Result<(), Sv2CpuMinerError> {
        miner
            .handle_setup_connection_success(
                None,
                SetupConnectionSuccessOwned {
                    used_version,
                    flags,
                },
                None,
            )
            .await
    }

    #[tokio::test]
    async fn accepts_the_offered_version_without_flags() {
        assert!(
            setup_success(&mut miner(1), SUPPORTED_PROTOCOL_VERSION, 0)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn rejects_a_version_that_was_not_offered() {
        assert!(matches!(
            setup_success(&mut miner(1), SUPPORTED_PROTOCOL_VERSION + 1, 0).await,
            Err(Sv2CpuMinerError::SetupConnectionMismatch(_))
        ));
    }

    #[tokio::test]
    async fn rejects_required_extended_channels_only_when_standard_channels_are_configured() {
        assert!(matches!(
            setup_success(
                &mut miner(1),
                SUPPORTED_PROTOCOL_VERSION,
                REQUIRES_EXTENDED_CHANNELS
            )
            .await,
            Err(Sv2CpuMinerError::SetupConnectionMismatch(_))
        ));
        assert!(
            setup_success(
                &mut miner(0),
                SUPPORTED_PROTOCOL_VERSION,
                REQUIRES_EXTENDED_CHANNELS
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn rejects_flags_the_spec_does_not_define() {
        assert!(matches!(
            setup_success(&mut miner(1), SUPPORTED_PROTOCOL_VERSION, 0b100).await,
            Err(Sv2CpuMinerError::SetupConnectionMismatch(_))
        ));
    }
}
