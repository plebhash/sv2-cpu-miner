//! Channel and group state for one connection, and the handling of every mining message.
//!
//! Every channel belongs to a group (sv2-spec 5.2.3). A message addressed to a group id fans
//! out to that group's members; one addressed to a channel id reaches that channel alone.

use crate::miner::format_number_with_underscores;
use std::collections::HashMap;
use stratum_apps::stratum_core::channels_sv2::client::group::GroupChannel;
use stratum_apps::stratum_core::mining_sv2::{
    OpenExtendedMiningChannelOwned, OpenStandardMiningChannelOwned,
};
use stratum_apps::stratum_core::parsers_sv2::MiningOwned;
use stratum_apps::utils::types::{Message, OutboundFrame};

use crate::miner::extended::ExtendedChannelMiner;
use crate::miner::standard::StandardChannelMiner;

use crate::error::Sv2CpuMinerError;
use tokio_util::sync::CancellationToken;

use tracing::info;

mod mining_message_handler;

/// Owns the open channels, their group membership and the mining task behind each channel.
pub struct ChannelManager {
    user_identity: String,
    nominal_hashrate: f32,
    nominal_hashrate_multiplier: f32,
    n_extended_channels: u8,
    n_standard_channels: u8,
    single_submit: bool,
    cpu_usage_percent: u64,
    requires_standard_jobs: bool,
    extended_channels: HashMap<u32, ExtendedChannelMiner>,
    standard_channels: HashMap<u32, StandardChannelMiner>,
    // every channel belongs to a group (spec 5.2.3); a server may run several groups on one
    // connection and redefine them with SetGroupChannel, so membership is tracked per group id
    // and server messages addressed to a group id fan out to that group's members only
    group_channels: HashMap<u32, GroupChannel>,
    upstream_sender: async_channel::Sender<OutboundFrame>,
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
        upstream_sender: async_channel::Sender<OutboundFrame>,
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
            upstream_sender,
            cancellation_token,
        }
    }

    /// Requests the configured standard and extended channels, splitting the advertised
    /// hashrate evenly between them. A channel exists once the mining server answers with
    /// `OpenMiningChannel.Success`.
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
            let frame = OutboundFrame::from_message(Message::Mining(
                MiningOwned::OpenStandardMiningChannel(open_standard_mining_channel),
            ))?;
            self.upstream_sender.send(frame).await?;
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
            let frame = OutboundFrame::from_message(Message::Mining(
                MiningOwned::OpenExtendedMiningChannel(open_extended_mining_channel),
            ))?;
            self.upstream_sender.send(frame).await?;
        }

        Ok(())
    }
}
