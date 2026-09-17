use crate::client::{Message, StdFrame, format_number_with_underscores};
use std::collections::HashMap;
use stratum_apps::stratum_core::channels_sv2::client::extended::ExtendedChannel;
use stratum_apps::stratum_core::channels_sv2::client::group::GroupChannel;
use stratum_apps::stratum_core::channels_sv2::client::standard::StandardChannel;
use stratum_apps::stratum_core::channels_sv2::extranonce_manager::ExtranoncePrefix;
use stratum_apps::stratum_core::common_messages_sv2::{
    ChannelEndpointChangedOwned, MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED, MESSAGE_TYPE_RECONNECT,
    ReconnectOwned, SetupConnectionErrorOwned, SetupConnectionSuccessOwned,
};
use stratum_apps::stratum_core::handlers_sv2::{
    HandleCommonMessagesFromServerOwnedAsync, HandleMiningMessagesFromServerOwnedAsync,
    HandlerErrorType, SupportedChannelTypes,
};
use stratum_apps::stratum_core::mining_sv2::{
    CloseChannelOwned, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS, NewExtendedMiningJobOwned, NewMiningJobOwned,
    OpenExtendedMiningChannelOwned, OpenExtendedMiningChannelSuccessOwned,
    OpenMiningChannelErrorOwned, OpenStandardMiningChannelOwned,
    OpenStandardMiningChannelSuccessOwned, SetCustomMiningJobErrorOwned,
    SetCustomMiningJobSuccessOwned, SetExtranoncePrefixOwned, SetGroupChannelOwned,
    SetNewPrevHashOwned, SetTargetOwned, SubmitSharesErrorOwned, SubmitSharesSuccessOwned,
    UpdateChannelErrorOwned,
};
use stratum_apps::stratum_core::parsers_sv2::{MiningOwned, Tlv};

use stratum_apps::stratum_core::bitcoin::Target;

use crate::miner::extended::ExtendedMiner;
use crate::miner::standard::StandardMiner;

use crate::error::Sv2CpuMinerError;
use tokio_util::sync::CancellationToken;

use tracing::{debug, error, info};

pub struct Sv2CpuMinerClientHandler {
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

impl Sv2CpuMinerClientHandler {
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

    /// Channels addressed by a server message: the members of `channel_id` when it names a
    /// group, otherwise `channel_id` itself. Group and channel ids share one namespace per
    /// connection (spec 5.2.3), so the lookup is unambiguous.
    fn addressed_channels(&self, channel_id: u32) -> Vec<u32> {
        match self.group_channels.get(&channel_id) {
            Some(group) => group.get_channel_ids().copied().collect(),
            None => vec![channel_id],
        }
    }

    /// Full extranonce size of an open channel, which every member of a group must share.
    async fn full_extranonce_size(&self, channel_id: u32) -> Option<usize> {
        if let Some(standard_miner) = self.standard_channels.get(&channel_id) {
            return Some(standard_miner.full_extranonce_size().await);
        }
        if let Some(extended_miner) = self.extended_channels.get(&channel_id) {
            return Some(extended_miner.full_extranonce_size().await);
        }
        None
    }

    /// Records a newly opened channel as a member of its group, creating the group on first use.
    ///
    /// Returns `true` when the channel is now a member and opening it may proceed. Returns
    /// `false` when the channel cannot be used on this connection: its ids collide with ids
    /// already in use (group and channel ids share one namespace, spec 5.2.3) or its full
    /// extranonce size differs from the group's (spec 5.2.3). The reason is logged here, so the
    /// caller only has to drop the OpenMiningChannel.Success it was handling.
    fn join_group(
        &mut self,
        channel_id: u32,
        group_channel_id: u32,
        full_extranonce_size: usize,
    ) -> bool {
        let reinterprets_channel = self.standard_channels.contains_key(&group_channel_id)
            || self.extended_channels.contains_key(&group_channel_id);
        if reinterprets_channel
            || channel_id == group_channel_id
            || self.group_channels.contains_key(&channel_id)
        {
            error!(
                "Channel ID: {} and Group Channel ID: {} collide with ids already in use on this connection",
                channel_id, group_channel_id
            );
            return false;
        }

        let group = self
            .group_channels
            .entry(group_channel_id)
            .or_insert_with(|| GroupChannel::new(group_channel_id));
        match group.add_channel_id(channel_id, full_extranonce_size) {
            Ok(()) => true,
            Err(e) => {
                error!(
                    "Channel ID: {} cannot join Group Channel ID: {}: {:?}",
                    channel_id, group_channel_id, e
                );
                self.group_channels.retain(|_, group| !group.is_empty());
                false
            }
        }
    }
}

impl HandleCommonMessagesFromServerOwnedAsync for Sv2CpuMinerClientHandler {
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

impl HandleMiningMessagesFromServerOwnedAsync for Sv2CpuMinerClientHandler {
    type Error = Sv2CpuMinerError;

    fn get_channel_type_for_server(&self, _server_id: Option<usize>) -> SupportedChannelTypes {
        SupportedChannelTypes::StandardAndExtended
    }

    fn is_work_selection_enabled_for_server(&self, _server_id: Option<usize>) -> bool {
        false
    }

    fn get_negotiated_extensions_with_server(
        &self,
        _server_id: Option<usize>,
    ) -> Result<Vec<u16>, Self::Error> {
        Ok(vec![])
    }

    async fn handle_open_standard_mining_channel_success(
        &mut self,
        _server_id: Option<usize>,
        open_standard_mining_channel_success: OpenStandardMiningChannelSuccessOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!(
            "Received OpenStandardMiningChannel.Success: {}",
            open_standard_mining_channel_success
        );

        let extranonce_prefix = match ExtranoncePrefix::from_wire(
            open_standard_mining_channel_success
                .extranonce_prefix
                .to_owned_bytes(),
        ) {
            Ok(extranonce_prefix) => extranonce_prefix,
            Err(e) => {
                error!(
                    "Invalid extranonce_prefix in OpenStandardMiningChannel.Success: {:?}",
                    e
                );
                return Ok(());
            }
        };

        let standard_channel = match StandardChannel::new(
            open_standard_mining_channel_success.channel_id,
            self.user_identity.clone(),
            extranonce_prefix,
            Target::from_le_bytes(open_standard_mining_channel_success.target.to_array()),
            self.nominal_hashrate / (self.n_standard_channels + self.n_extended_channels) as f32,
            None,
        ) {
            Ok(standard_channel) => standard_channel,
            Err(e) => {
                error!("Failed to create Standard Channel: {:?}", e);
                return Ok(());
            }
        };

        debug!("Created new Standard Channel: {:?}", standard_channel);

        if !self.join_group(
            open_standard_mining_channel_success.channel_id,
            open_standard_mining_channel_success.group_channel_id,
            open_standard_mining_channel_success.extranonce_prefix.len(),
        ) {
            return Ok(());
        }

        self.standard_channels.insert(
            open_standard_mining_channel_success.channel_id,
            StandardMiner::new(
                standard_channel,
                self.cpu_usage_percent,
                self.single_submit,
                self.event_injector.clone(),
                self.cancellation_token.clone(),
            ),
        );

        Ok(())
    }

    async fn handle_open_extended_mining_channel_success(
        &mut self,
        _server_id: Option<usize>,
        open_extended_mining_channel_success: OpenExtendedMiningChannelSuccessOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!(
            "Received OpenExtendedMiningChannel.Success: {}",
            open_extended_mining_channel_success
        );

        let extranonce_prefix = match ExtranoncePrefix::from_wire(
            open_extended_mining_channel_success
                .extranonce_prefix
                .to_owned_bytes(),
        ) {
            Ok(extranonce_prefix) => extranonce_prefix,
            Err(e) => {
                error!(
                    "Invalid extranonce_prefix in OpenExtendedMiningChannel.Success: {:?}",
                    e
                );
                return Ok(());
            }
        };

        let extended_channel = match ExtendedChannel::new(
            open_extended_mining_channel_success.channel_id,
            self.user_identity.clone(),
            extranonce_prefix,
            Target::from_le_bytes(open_extended_mining_channel_success.target.to_array()),
            self.nominal_hashrate / (self.n_standard_channels + self.n_extended_channels) as f32,
            true,
            open_extended_mining_channel_success.extranonce_size,
            None,
        ) {
            Ok(extended_channel) => extended_channel,
            Err(e) => {
                error!("Failed to create Extended Channel: {:?}", e);
                return Ok(());
            }
        };

        debug!("Created new Extended Channel: {:?}", extended_channel);

        if !self.join_group(
            open_extended_mining_channel_success.channel_id,
            open_extended_mining_channel_success.group_channel_id,
            open_extended_mining_channel_success.extranonce_prefix.len()
                + open_extended_mining_channel_success.extranonce_size as usize,
        ) {
            return Ok(());
        }

        self.extended_channels.insert(
            open_extended_mining_channel_success.channel_id,
            ExtendedMiner::new(
                extended_channel,
                self.cpu_usage_percent,
                self.single_submit,
                self.event_injector.clone(),
                self.cancellation_token.clone(),
            ),
        );

        Ok(())
    }

    async fn handle_open_mining_channel_error(
        &mut self,
        _server_id: Option<usize>,
        open_standard_mining_channel_error: OpenMiningChannelErrorOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!(
            "Received OpenMiningChannel.Error: {}",
            open_standard_mining_channel_error
        );
        Ok(())
    }

    async fn handle_update_channel_error(
        &mut self,
        _server_id: Option<usize>,
        update_channel_error: UpdateChannelErrorOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received UpdateChannel.Error: {}", update_channel_error);
        Ok(())
    }

    async fn handle_close_channel(
        &mut self,
        _server_id: Option<usize>,
        close_channel: CloseChannelOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received CloseChannel: {}", close_channel);

        for channel_id in self.addressed_channels(close_channel.channel_id) {
            if self.standard_channels.remove(&channel_id).is_some() {
                info!("Removed Standard Channel with ID: {}", channel_id);
            } else if self.extended_channels.remove(&channel_id).is_some() {
                info!("Removed Extended Channel with ID: {}", channel_id);
            } else {
                error!(
                    "Channel with ID: {} not found, ignoring CloseChannel.",
                    channel_id
                );
                continue;
            }
            for group in self.group_channels.values_mut() {
                group.remove_channel_id(channel_id);
            }
        }
        self.group_channels.retain(|_, group| !group.is_empty());

        Ok(())
    }

    async fn handle_set_extranonce_prefix(
        &mut self,
        _server_id: Option<usize>,
        set_extranonce_prefix: SetExtranoncePrefixOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("received SetExtranoncePrefix: {}", set_extranonce_prefix);

        // ExtranoncePrefix is not Clone, so mint one per branch from the wire bytes
        let extranonce_prefix_bytes = set_extranonce_prefix.extranonce_prefix.to_owned_bytes();
        let extranonce_prefix = match ExtranoncePrefix::from_wire(extranonce_prefix_bytes.clone()) {
            Ok(extranonce_prefix) => extranonce_prefix,
            Err(e) => {
                error!("Invalid extranonce_prefix in SetExtranoncePrefix: {:?}", e);
                return Ok(());
            }
        };

        let has_standard_channel = self
            .standard_channels
            .contains_key(&set_extranonce_prefix.channel_id);
        let has_extended_channel = self
            .extended_channels
            .contains_key(&set_extranonce_prefix.channel_id);

        if has_standard_channel {
            let standard_channel = self
                .standard_channels
                .get_mut(&set_extranonce_prefix.channel_id)
                .expect("channel id must exist");

            match standard_channel
                .set_extranonce_prefix(extranonce_prefix)
                .await
            {
                Ok(()) => {
                    info!(
                        "updated standard channel with id: {}, new extranonce prefix: {}",
                        set_extranonce_prefix.channel_id, set_extranonce_prefix.extranonce_prefix
                    );
                }
                Err(e) => {
                    error!(
                        "failed to set new extranonce prefix for standard channel with id: {}, error: {:?}",
                        set_extranonce_prefix.channel_id, e
                    );
                }
            };
        }

        if has_extended_channel {
            let extended_channel = self
                .extended_channels
                .get_mut(&set_extranonce_prefix.channel_id)
                .expect("channel id must exist");

            let extranonce_prefix =
                match ExtranoncePrefix::from_wire(extranonce_prefix_bytes.clone()) {
                    Ok(extranonce_prefix) => extranonce_prefix,
                    Err(e) => {
                        error!("Invalid extranonce_prefix in SetExtranoncePrefix: {:?}", e);
                        return Ok(());
                    }
                };

            match extended_channel
                .set_extranonce_prefix(extranonce_prefix)
                .await
            {
                Ok(()) => {
                    info!(
                        "updated extended channel with id: {}, new extranonce prefix: {}",
                        set_extranonce_prefix.channel_id, set_extranonce_prefix.extranonce_prefix
                    );
                }
                Err(e) => {
                    error!(
                        "failed to set new extranonce prefix for extended channel with id: {}, error: {:?}",
                        set_extranonce_prefix.channel_id, e
                    );
                }
            }
        }

        if !has_standard_channel && !has_extended_channel {
            error!(
                "Channel with ID: {} not found, ignoring SetExtranoncePrefix.",
                set_extranonce_prefix.channel_id
            );
        }

        Ok(())
    }

    async fn handle_submit_shares_success(
        &mut self,
        _server_id: Option<usize>,
        submit_shares_success: SubmitSharesSuccessOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("received SubmitShares.Success: {}", submit_shares_success);
        Ok(())
    }

    async fn handle_submit_shares_error(
        &mut self,
        _server_id: Option<usize>,
        submit_shares_error: SubmitSharesErrorOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("received SubmitShares.Error: {}", submit_shares_error);
        Ok(())
    }

    async fn handle_new_mining_job(
        &mut self,
        _server_id: Option<usize>,
        new_mining_job: NewMiningJobOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received NewMiningJob: {}", new_mining_job);

        match self.standard_channels.get_mut(&new_mining_job.channel_id) {
            None => {
                error!(
                    "Standard Channel ID: {} not found. Ignoring NewMiningJob.",
                    new_mining_job.channel_id
                );
            }
            Some(standard_channel) => {
                match standard_channel
                    .on_new_mining_job(new_mining_job.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "NewMiningJob processed: Standard Channel ID: {}, Job ID: {}",
                            new_mining_job.channel_id, new_mining_job.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process NewMiningJob for Standard Channel with ID: {}, error: {:?}",
                            new_mining_job.channel_id, e
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_new_extended_mining_job(
        &mut self,
        _server_id: Option<usize>,
        new_extended_mining_job: NewExtendedMiningJobOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received NewExtendedMiningJob: {}", new_extended_mining_job);

        if self.requires_standard_jobs {
            return Err(Sv2CpuMinerError::StandardJobsOnly("NewExtendedMiningJob"));
        }

        let channel_id = new_extended_mining_job.channel_id;
        let job_id = new_extended_mining_job.job_id;

        let Some(group) = self.group_channels.get(&channel_id) else {
            // addressed to one extended channel; a standard channel only ever receives an
            // extended job through its group
            match self.extended_channels.get_mut(&channel_id) {
                None => {
                    error!(
                        "Extended Channel ID: {} not found. Ignoring NewExtendedMiningJob.",
                        channel_id
                    );
                }
                Some(extended_miner) => {
                    match extended_miner
                        .on_new_extended_mining_job(new_extended_mining_job)
                        .await
                    {
                        Ok(()) => {
                            info!(
                                "NewExtendedMiningJob processed: Extended Channel ID: {}, Job ID: {}",
                                channel_id, job_id
                            );
                        }
                        Err(e) => {
                            error!(
                                "Failed to process NewExtendedMiningJob for Extended Channel with ID: {}, error: {:?}",
                                channel_id, e
                            );
                        }
                    }
                }
            }
            return Ok(());
        };

        // group broadcast: standard members derive their own merkle root from the extended job
        let members: Vec<u32> = group.get_channel_ids().copied().collect();
        for member_id in members {
            if let Some(standard_miner) = self.standard_channels.get_mut(&member_id) {
                match standard_miner
                    .on_group_channel_job(new_extended_mining_job.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "NewExtendedMiningJob processed: Group Channel ID: {}, Standard Channel ID: {}, Job ID: {}",
                            channel_id, member_id, job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process group NewExtendedMiningJob for Standard Channel with ID: {}, error: {:?}",
                            member_id, e
                        );
                    }
                }
            } else if let Some(extended_miner) = self.extended_channels.get_mut(&member_id) {
                match extended_miner
                    .on_new_extended_mining_job(new_extended_mining_job.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "NewExtendedMiningJob processed: Group Channel ID: {}, Extended Channel ID: {}, Job ID: {}",
                            channel_id, member_id, job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process group NewExtendedMiningJob for Extended Channel with ID: {}, error: {:?}",
                            member_id, e
                        );
                    }
                }
            } else {
                error!(
                    "Group Channel ID: {} lists unknown Channel ID: {}. Ignoring NewExtendedMiningJob for it.",
                    channel_id, member_id
                );
            }
        }

        Ok(())
    }

    async fn handle_set_new_prev_hash(
        &mut self,
        _server_id: Option<usize>,
        set_new_prev_hash: SetNewPrevHashOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received SetNewPrevHash: {}", set_new_prev_hash);

        for channel_id in self.addressed_channels(set_new_prev_hash.channel_id) {
            if let Some(standard_miner) = self.standard_channels.get_mut(&channel_id) {
                match standard_miner
                    .on_set_new_prev_hash(set_new_prev_hash.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "SetNewPrevHash processed: Standard Channel ID: {}, Job ID: {}",
                            channel_id, set_new_prev_hash.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process SetNewPrevHash for Standard Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            } else if let Some(extended_miner) = self.extended_channels.get_mut(&channel_id) {
                match extended_miner
                    .on_set_new_prev_hash(set_new_prev_hash.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "SetNewPrevHash processed: Extended Channel ID: {}, Job ID: {}",
                            channel_id, set_new_prev_hash.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process SetNewPrevHash for Extended Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            } else {
                error!(
                    "Channel with ID: {} not found, ignoring SetNewPrevHash.",
                    channel_id
                );
            }
        }

        Ok(())
    }

    async fn handle_set_custom_mining_job_success(
        &mut self,
        _server_id: Option<usize>,
        _set_custom_mining_job_success: SetCustomMiningJobSuccessOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received unexpected SetCustomMiningJob.Success");
        Err(Sv2CpuMinerError::unexpected_message(
            0,
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
        ))
    }

    async fn handle_set_custom_mining_job_error(
        &mut self,
        _server_id: Option<usize>,
        _set_custom_mining_job_error: SetCustomMiningJobErrorOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received unexpected SetCustomMiningJob.Error");
        Err(Sv2CpuMinerError::unexpected_message(
            0,
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
        ))
    }

    async fn handle_set_target(
        &mut self,
        _server_id: Option<usize>,
        set_target: SetTargetOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received SetTarget: {}", set_target);

        let target = Target::from_le_bytes(set_target.maximum_target.to_array());

        for channel_id in self.addressed_channels(set_target.channel_id) {
            if let Some(standard_miner) = self.standard_channels.get_mut(&channel_id) {
                match standard_miner.set_target(target).await {
                    Ok(()) => {
                        info!("SetTarget processed: Standard Channel ID: {}", channel_id);
                    }
                    Err(e) => {
                        error!(
                            "Failed to process SetTarget for Standard Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            } else if let Some(extended_miner) = self.extended_channels.get_mut(&channel_id) {
                match extended_miner.set_target(target).await {
                    Ok(()) => {
                        info!("SetTarget processed: Extended Channel ID: {}", channel_id);
                    }
                    Err(e) => {
                        error!(
                            "Failed to process SetTarget for Extended Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            } else {
                error!(
                    "Channel with ID: {} not found, ignoring SetTarget.",
                    channel_id
                );
            }
        }

        Ok(())
    }

    async fn handle_set_group_channel(
        &mut self,
        _server_id: Option<usize>,
        set_group_channel: SetGroupChannelOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received SetGroupChannel: {}", set_group_channel);

        if self.requires_standard_jobs {
            return Err(Sv2CpuMinerError::StandardJobsOnly("SetGroupChannel"));
        }

        let group_channel_id = set_group_channel.group_channel_id;
        let channel_ids = set_group_channel.channel_ids.into_inner();

        if self.standard_channels.contains_key(&group_channel_id)
            || self.extended_channels.contains_key(&group_channel_id)
        {
            error!(
                "SetGroupChannel reinterprets open Channel ID: {} as a group channel, ignoring.",
                group_channel_id
            );
            return Ok(());
        }

        // validate the whole redefinition before touching any group
        let mut redefined_group = GroupChannel::new(group_channel_id);
        for &channel_id in &channel_ids {
            let Some(full_extranonce_size) = self.full_extranonce_size(channel_id).await else {
                error!(
                    "SetGroupChannel lists unknown Channel ID: {}, ignoring.",
                    channel_id
                );
                return Ok(());
            };
            if let Err(e) = redefined_group.add_channel_id(channel_id, full_extranonce_size) {
                error!(
                    "Channel ID: {} cannot join Group Channel ID: {}: {:?}, ignoring SetGroupChannel.",
                    channel_id, group_channel_id, e
                );
                return Ok(());
            }
        }

        // the listed channels leave their previous groups and the target group is redefined as
        // exactly the listed channels, matching the sv2-apps translator
        for group in self.group_channels.values_mut() {
            for channel_id in &channel_ids {
                group.remove_channel_id(*channel_id);
            }
        }
        self.group_channels.remove(&group_channel_id);
        if !channel_ids.is_empty() {
            self.group_channels
                .insert(group_channel_id, redefined_group);
        }
        self.group_channels.retain(|_, group| !group.is_empty());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stratum_apps::stratum_core::binary_sv2::Sv2OptionOwned;

    fn handler(
        requires_standard_jobs: bool,
    ) -> (Sv2CpuMinerClientHandler, async_channel::Receiver<StdFrame>) {
        let (event_injector, receiver) = async_channel::unbounded();
        let handler = Sv2CpuMinerClientHandler::new(
            "user".to_string(),
            1000.0,
            1.0,
            0,
            3,
            false,
            100,
            requires_standard_jobs,
            event_injector,
            CancellationToken::new(),
        );
        (handler, receiver)
    }

    fn open_standard_success(
        channel_id: u32,
        group_channel_id: u32,
    ) -> OpenStandardMiningChannelSuccessOwned {
        OpenStandardMiningChannelSuccessOwned {
            request_id: channel_id,
            channel_id,
            target: [0xFF_u8; 32].into(),
            // 32 bytes: the group job fixture's coinbase carries a 32-byte extranonce
            extranonce_prefix: vec![0_u8; 32].try_into().unwrap(),
            group_channel_id,
        }
    }

    /// Future job whose coinbase parses once a 32-byte extranonce is inserted; the bytes come
    /// from the channels_sv2 client tests.
    fn group_job(channel_id: u32, job_id: u32) -> NewExtendedMiningJobOwned {
        NewExtendedMiningJobOwned {
            channel_id,
            job_id,
            min_ntime: Sv2OptionOwned::new(None),
            version: 536870912,
            version_rolling_allowed: true,
            merkle_path: vec![].try_into().unwrap(),
            coinbase_tx_prefix: vec![
                2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 34, 82, 0,
            ]
            .try_into()
            .unwrap(),
            coinbase_tx_suffix: vec![
                255, 255, 255, 255, 2, 0, 242, 5, 42, 1, 0, 0, 0, 22, 0, 20, 235, 225, 183, 220,
                194, 147, 204, 170, 14, 231, 67, 168, 111, 137, 223, 130, 88, 194, 8, 252, 0, 0, 0,
                0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 226, 246, 28, 63, 113, 209, 222,
                253, 63, 169, 153, 223, 163, 105, 83, 117, 92, 105, 6, 137, 121, 153, 98, 180, 139,
                235, 216, 54, 151, 78, 140, 249, 0, 0, 0, 0,
            ]
            .try_into()
            .unwrap(),
        }
    }

    /// Opens standard channels 2 and 3 in group 1 and channel 5 in group 4.
    async fn handler_with_two_groups()
    -> (Sv2CpuMinerClientHandler, async_channel::Receiver<StdFrame>) {
        let (mut handler, receiver) = handler(false);
        for (channel_id, group_channel_id) in [(2, 1), (3, 1), (5, 4)] {
            handler
                .handle_open_standard_mining_channel_success(
                    None,
                    open_standard_success(channel_id, group_channel_id),
                    None,
                )
                .await
                .unwrap();
        }
        (handler, receiver)
    }

    async fn future_job_ids(handler: &Sv2CpuMinerClientHandler, channel_id: u32) -> Vec<u32> {
        let mut job_ids = handler.standard_channels[&channel_id]
            .future_job_ids()
            .await;
        job_ids.sort_unstable();
        job_ids
    }

    #[tokio::test]
    async fn group_job_reaches_only_the_members_of_that_group() {
        let (mut handler, _receiver) = handler_with_two_groups().await;

        handler
            .handle_new_extended_mining_job(None, group_job(1, 10), None)
            .await
            .unwrap();
        handler
            .handle_new_extended_mining_job(None, group_job(4, 40), None)
            .await
            .unwrap();

        assert_eq!(future_job_ids(&handler, 2).await, vec![10]);
        assert_eq!(future_job_ids(&handler, 3).await, vec![10]);
        assert_eq!(future_job_ids(&handler, 5).await, vec![40]);
    }

    #[tokio::test]
    async fn extended_job_sent_to_a_standard_channel_is_ignored() {
        let (mut handler, _receiver) = handler_with_two_groups().await;

        handler
            .handle_new_extended_mining_job(None, group_job(2, 10), None)
            .await
            .unwrap();

        assert!(future_job_ids(&handler, 2).await.is_empty());
    }

    #[tokio::test]
    async fn set_group_channel_redefines_membership() {
        let (mut handler, _receiver) = handler_with_two_groups().await;

        // group 4 becomes {3, 5}; channel 3 leaves group 1
        handler
            .handle_set_group_channel(
                None,
                SetGroupChannelOwned {
                    group_channel_id: 4,
                    channel_ids: vec![3, 5].try_into().unwrap(),
                },
                None,
            )
            .await
            .unwrap();
        handler
            .handle_new_extended_mining_job(None, group_job(1, 10), None)
            .await
            .unwrap();
        handler
            .handle_new_extended_mining_job(None, group_job(4, 40), None)
            .await
            .unwrap();

        assert_eq!(future_job_ids(&handler, 2).await, vec![10]);
        assert_eq!(future_job_ids(&handler, 3).await, vec![40]);
        assert_eq!(future_job_ids(&handler, 5).await, vec![40]);
    }

    #[tokio::test]
    async fn set_group_channel_reusing_a_channel_id_is_ignored() {
        let (mut handler, _receiver) = handler_with_two_groups().await;

        handler
            .handle_set_group_channel(
                None,
                SetGroupChannelOwned {
                    group_channel_id: 2,
                    channel_ids: vec![3].try_into().unwrap(),
                },
                None,
            )
            .await
            .unwrap();
        handler
            .handle_new_extended_mining_job(None, group_job(1, 10), None)
            .await
            .unwrap();

        assert_eq!(future_job_ids(&handler, 3).await, vec![10]);
        assert!(!handler.group_channels.contains_key(&2));
    }

    #[tokio::test]
    async fn close_channel_addressed_to_a_group_closes_its_members() {
        let (mut handler, _receiver) = handler_with_two_groups().await;

        handler
            .handle_close_channel(
                None,
                CloseChannelOwned {
                    channel_id: 1,
                    reason_code: "test".to_string().try_into().unwrap(),
                },
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            handler
                .standard_channels
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![5]
        );
        assert_eq!(
            handler.group_channels.keys().copied().collect::<Vec<_>>(),
            vec![4]
        );
    }

    #[tokio::test]
    async fn extended_job_on_a_standard_jobs_connection_is_fatal() {
        let (mut handler, _receiver) = handler(true);
        handler
            .handle_open_standard_mining_channel_success(None, open_standard_success(2, 1), None)
            .await
            .unwrap();

        assert!(matches!(
            handler
                .handle_new_extended_mining_job(None, group_job(1, 10), None)
                .await,
            Err(Sv2CpuMinerError::StandardJobsOnly("NewExtendedMiningJob"))
        ));
        assert!(future_job_ids(&handler, 2).await.is_empty());
    }
}
