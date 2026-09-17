use crate::client::{Message, StdFrame, format_number_with_underscores};
use std::collections::HashMap;
use stratum_apps::stratum_core::channels_sv2::client::group::GroupChannel;
use stratum_apps::stratum_core::common_messages_sv2::{
    ChannelEndpointChangedOwned, MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED, MESSAGE_TYPE_RECONNECT,
    ReconnectOwned, SetupConnectionErrorOwned, SetupConnectionSuccessOwned,
};
use stratum_apps::stratum_core::handlers_sv2::{
    HandleCommonMessagesFromServerOwnedAsync, HandlerErrorType,
};
use stratum_apps::stratum_core::mining_sv2::{
    OpenExtendedMiningChannelOwned, OpenStandardMiningChannelOwned,
};
use stratum_apps::stratum_core::parsers_sv2::{MiningOwned, Tlv};

use crate::miner::extended::ExtendedMiner;
use crate::miner::standard::StandardMiner;

use crate::error::Sv2CpuMinerError;
use tokio_util::sync::CancellationToken;

use tracing::{error, info};

mod mining_message_handler;

pub struct ChannelManager {
    user_identity: String,
    nominal_hashrate: f32,
    nominal_hashrate_multiplier: f32,
    n_extended_channels: u8,
    n_standard_channels: u8,
    single_submit: bool,
    cpu_usage_percent: u64,
    requires_standard_jobs: bool,
    extended_channels: HashMap<u32, ExtendedMiner>,
    standard_channels: HashMap<u32, StandardMiner>,
    // every channel belongs to a group (spec 5.2.3); a server may run several groups on one
    // connection and redefine them with SetGroupChannel, so membership is tracked per group id
    // and server messages addressed to a group id fan out to that group's members only
    group_channels: HashMap<u32, GroupChannel>,
    event_injector: async_channel::Sender<StdFrame>,
    cancellation_token: CancellationToken,
}

impl ChannelManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_identity: String,
        nominal_hashrate: f32,
        nominal_hashrate_multiplier: f32,
        n_extended_channels: u8,
        n_standard_channels: u8,
        single_submit: bool,
        cpu_usage_percent: u64,
        requires_standard_jobs: bool,
        event_injector: async_channel::Sender<StdFrame>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            user_identity,
            nominal_hashrate,
            nominal_hashrate_multiplier,
            n_extended_channels,
            n_standard_channels,
            single_submit,
            cpu_usage_percent,
            requires_standard_jobs,
            extended_channels: HashMap::with_capacity(n_extended_channels as usize),
            standard_channels: HashMap::with_capacity(n_standard_channels as usize),
            group_channels: HashMap::new(),
            event_injector,
            cancellation_token,
        }
    }

    pub async fn open_channels(&mut self) -> Result<(), Sv2CpuMinerError> {
        let nominal_hashrate_per_channel = (self.nominal_hashrate
            * self.nominal_hashrate_multiplier)
            / (self.n_standard_channels + self.n_extended_channels) as f32;

        for i in 0..self.n_standard_channels {
            info!(
                "Sending OpenStandardMiningChannel with nominal hashrate: {} H/s",
                format_number_with_underscores(nominal_hashrate_per_channel as u64)
            );
            let open_standard_mining_channel = OpenStandardMiningChannelOwned {
                request_id: i as u32,
                user_identity: self
                    .user_identity
                    .clone()
                    .try_into()
                    .expect("user_identity length checked at config load"),
                nominal_hash_rate: nominal_hashrate_per_channel,
                max_target: [0xFF_u8; 32].into(), // allow maximum possible target
            };
            let frame: StdFrame = Message::Mining(MiningOwned::OpenStandardMiningChannel(
                open_standard_mining_channel,
            ))
            .try_into()
            .expect("OpenStandardMiningChannel must be serializable");
            self.event_injector.send(frame).await?;
        }

        for i in 0..self.n_extended_channels {
            info!(
                "Sending OpenExtendedMiningChannel with nominal hashrate: {} H/s",
                format_number_with_underscores(nominal_hashrate_per_channel as u64)
            );
            let open_extended_mining_channel = OpenExtendedMiningChannelOwned {
                request_id: (i + self.n_standard_channels) as u32,
                user_identity: self
                    .user_identity
                    .clone()
                    .try_into()
                    .expect("user_identity length checked at config load"),
                nominal_hash_rate: nominal_hashrate_per_channel,
                max_target: [0xFF_u8; 32].into(), // allow maximum possible target
                min_extranonce_size: 0, // no extranonce rolling to avoid merkle root calculation overhead
            };
            let frame: StdFrame = Message::Mining(MiningOwned::OpenExtendedMiningChannel(
                open_extended_mining_channel,
            ))
            .try_into()
            .expect("OpenExtendedMiningChannel must be serializable");
            self.event_injector.send(frame).await?;
        }

        Ok(())
    }
}

impl HandleCommonMessagesFromServerOwnedAsync for ChannelManager {
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
