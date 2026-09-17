use crate::client::{Message, StdFrame, format_number_with_underscores};
use std::collections::HashMap;
use stratum_apps::stratum_core::channels_sv2::client::extended::ExtendedChannel;
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
    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS, MESSAGE_TYPE_SET_GROUP_CHANNEL,
    NewExtendedMiningJobOwned, NewMiningJobOwned, OpenExtendedMiningChannelOwned,
    OpenExtendedMiningChannelSuccessOwned, OpenMiningChannelErrorOwned,
    OpenStandardMiningChannelOwned, OpenStandardMiningChannelSuccessOwned,
    SetCustomMiningJobErrorOwned, SetCustomMiningJobSuccessOwned, SetExtranoncePrefixOwned,
    SetGroupChannelOwned, SetNewPrevHashOwned, SetTargetOwned, SubmitSharesErrorOwned,
    SubmitSharesSuccessOwned, UpdateChannelErrorOwned,
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
    extended_channels: HashMap<u32, ExtendedMiner>,
    standard_channels: HashMap<u32, StandardMiner>,
    // on connections without REQUIRES_STANDARD_JOBS, the pool groups all channels and
    // addresses subsequent NewExtendedMiningJob/SetNewPrevHash to this group channel id
    group_channel_id: Option<u32>,
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
            extended_channels: HashMap::with_capacity(n_extended_channels as usize),
            standard_channels: HashMap::with_capacity(n_standard_channels as usize),
            group_channel_id: None,
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
                    .expect("user_identity must fit in Str0255"),
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
                    .expect("user_identity must fit in Str0255"),
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

        self.group_channel_id = Some(open_standard_mining_channel_success.group_channel_id);

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

        self.group_channel_id = Some(open_extended_mining_channel_success.group_channel_id);

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

        let has_standard_channel = self
            .standard_channels
            .contains_key(&close_channel.channel_id);
        let has_extended_channel = self
            .extended_channels
            .contains_key(&close_channel.channel_id);

        if has_standard_channel {
            self.standard_channels.remove(&close_channel.channel_id);
            info!(
                "Removed Standard Channel with ID: {}",
                close_channel.channel_id
            );
        }

        if has_extended_channel {
            self.extended_channels.remove(&close_channel.channel_id);
            info!(
                "Removed Extended Channel with ID: {}",
                close_channel.channel_id
            );
        }

        if !has_standard_channel && !has_extended_channel {
            error!(
                "Channel with ID: {} not found, ignoring CloseChannel.",
                close_channel.channel_id
            );
        }

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

        // group-addressed job: applies to every channel on this connection
        if Some(new_extended_mining_job.channel_id) == self.group_channel_id {
            for (channel_id, standard_miner) in self.standard_channels.iter_mut() {
                match standard_miner
                    .on_group_channel_job(new_extended_mining_job.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "NewExtendedMiningJob processed: Group Channel ID: {}, Standard Channel ID: {}, Job ID: {}",
                            new_extended_mining_job.channel_id,
                            channel_id,
                            new_extended_mining_job.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process group NewExtendedMiningJob for Standard Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            }
            for (channel_id, extended_miner) in self.extended_channels.iter_mut() {
                match extended_miner
                    .on_new_extended_mining_job(new_extended_mining_job.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "NewExtendedMiningJob processed: Group Channel ID: {}, Extended Channel ID: {}, Job ID: {}",
                            new_extended_mining_job.channel_id,
                            channel_id,
                            new_extended_mining_job.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process group NewExtendedMiningJob for Extended Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            }
            return Ok(());
        }

        match self
            .extended_channels
            .get_mut(&new_extended_mining_job.channel_id)
        {
            None => {
                error!(
                    "Extended Channel ID: {} not found. Ignoring NewExtendedMiningJob.",
                    new_extended_mining_job.channel_id
                );
            }
            Some(extended_channel) => {
                match extended_channel
                    .on_new_extended_mining_job(new_extended_mining_job.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "NewExtendedMiningJob processed: Extended Channel ID: {:?}, Job ID: {:?}",
                            new_extended_mining_job.channel_id, new_extended_mining_job.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process NewExtendedMiningJob for Extended Channel with ID: {}, error: {:?}",
                            new_extended_mining_job.channel_id, e
                        );
                    }
                }
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

        // group-addressed prev hash: applies to every channel on this connection
        if Some(set_new_prev_hash.channel_id) == self.group_channel_id {
            for (channel_id, standard_miner) in self.standard_channels.iter_mut() {
                match standard_miner
                    .on_set_new_prev_hash(set_new_prev_hash.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "SetNewPrevHash processed: Group Channel ID: {}, Standard Channel ID: {}, Job ID: {}",
                            set_new_prev_hash.channel_id, channel_id, set_new_prev_hash.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process group SetNewPrevHash for Standard Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            }
            for (channel_id, extended_miner) in self.extended_channels.iter_mut() {
                match extended_miner
                    .on_set_new_prev_hash(set_new_prev_hash.clone())
                    .await
                {
                    Ok(()) => {
                        info!(
                            "SetNewPrevHash processed: Group Channel ID: {}, Extended Channel ID: {}, Job ID: {}",
                            set_new_prev_hash.channel_id, channel_id, set_new_prev_hash.job_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to process group SetNewPrevHash for Extended Channel with ID: {}, error: {:?}",
                            channel_id, e
                        );
                    }
                }
            }
            return Ok(());
        }

        let has_standard_channel = self
            .standard_channels
            .contains_key(&set_new_prev_hash.channel_id);
        let has_extended_channel = self
            .extended_channels
            .contains_key(&set_new_prev_hash.channel_id);

        if !has_standard_channel && !has_extended_channel {
            error!(
                "Channel with ID: {} not found, ignoring SetNewPrevHash.",
                set_new_prev_hash.channel_id
            );
        }

        if has_standard_channel {
            let standard_channel = self
                .standard_channels
                .get_mut(&set_new_prev_hash.channel_id)
                .expect("channel id must exist");

            match standard_channel
                .on_set_new_prev_hash(set_new_prev_hash.clone())
                .await
            {
                Ok(()) => {
                    info!(
                        "SetNewPrevHash processed: Standard Channel ID: {}, Job ID: {}",
                        set_new_prev_hash.channel_id, set_new_prev_hash.job_id
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to process SetNewPrevHash for Standard Channel with ID: {}, error: {:?}",
                        set_new_prev_hash.channel_id, e
                    );
                }
            };
        }

        if has_extended_channel {
            let extended_channel = self
                .extended_channels
                .get_mut(&set_new_prev_hash.channel_id)
                .expect("channel id must exist");

            match extended_channel
                .on_set_new_prev_hash(set_new_prev_hash.clone())
                .await
            {
                Ok(()) => {
                    info!(
                        "SetNewPrevHash processed: Extended Channel ID: {}, Job ID: {}",
                        set_new_prev_hash.channel_id, set_new_prev_hash.job_id
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to process SetNewPrevHash for Extended Channel with ID: {}, error: {:?}",
                        set_new_prev_hash.channel_id, e
                    );
                }
            };
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

        let has_standard_channel = self.standard_channels.contains_key(&set_target.channel_id);
        let has_extended_channel = self.extended_channels.contains_key(&set_target.channel_id);

        if !has_standard_channel && !has_extended_channel {
            error!(
                "Channel with ID: {} not found, ignoring SetTarget.",
                set_target.channel_id
            );
        }

        if has_standard_channel {
            let standard_channel = self
                .standard_channels
                .get_mut(&set_target.channel_id)
                .expect("channel id must exist");

            match standard_channel.set_target(target).await {
                Ok(()) => {
                    info!(
                        "SetTarget processed: Standard Channel ID: {}",
                        set_target.channel_id
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to process SetTarget for Standard Channel with ID: {}, error: {:?}",
                        set_target.channel_id, e
                    );
                }
            }
        }

        if has_extended_channel {
            let extended_channel = self
                .extended_channels
                .get_mut(&set_target.channel_id)
                .expect("channel id must exist");

            match extended_channel.set_target(target).await {
                Ok(()) => {
                    info!(
                        "SetTarget processed: Extended Channel ID: {}",
                        set_target.channel_id
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to process SetTarget for Extended Channel with ID: {}, error: {:?}",
                        set_target.channel_id, e
                    );
                }
            }
        }

        Ok(())
    }

    async fn handle_set_group_channel(
        &mut self,
        _server_id: Option<usize>,
        _set_group_channel: SetGroupChannelOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received unexpected SetGroupChannel");
        Err(Sv2CpuMinerError::unexpected_message(
            0,
            MESSAGE_TYPE_SET_GROUP_CHANNEL,
        ))
    }
}
