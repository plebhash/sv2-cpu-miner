use crate::client::format_number_with_underscores;
use std::collections::HashMap;
use sv2_services::client::service::event::Sv2ClientEvent;
use sv2_services::client::service::event::Sv2ClientEventError;
use sv2_services::client::service::outcome::Sv2ClientOutcome;
use sv2_services::client::service::subprotocols::mining::handler::Sv2MiningClientHandler;
use sv2_services::client::service::subprotocols::mining::trigger::MiningClientTrigger;
use sv2_services::roles_logic_sv2::channels::client::extended::ExtendedChannel;
use sv2_services::roles_logic_sv2::channels::client::standard::StandardChannel;
use sv2_services::roles_logic_sv2::mining_sv2::{
    CloseChannel, NewExtendedMiningJob, NewMiningJob, OpenExtendedMiningChannelSuccess,
    OpenMiningChannelError, OpenStandardMiningChannelSuccess, SetCustomMiningJobError,
    SetCustomMiningJobSuccess, SetExtranoncePrefix, SetGroupChannel, SetNewPrevHash, SetTarget,
    SubmitSharesError, SubmitSharesSuccess, UpdateChannelError,
};

use crate::miner::extended::ExtendedMiner;
use crate::miner::standard::StandardMiner;

use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use tracing::{debug, error, info};

#[derive(Clone)]
pub struct Sv2CpuMinerClientHandler {
    user_identity: String,
    nominal_hashrate: f32,
    nominal_hashrate_multiplier: f32,
    n_extended_channels: u8,
    n_standard_channels: u8,
    single_submit: bool,
    cpu_usage_percent: u64,
    extended_channels: Arc<RwLock<HashMap<u32, Arc<RwLock<ExtendedMiner>>>>>,
    standard_channels: Arc<RwLock<HashMap<u32, Arc<RwLock<StandardMiner>>>>>,
    event_injector: async_channel::Sender<Sv2ClientEvent<'static>>,
    cancellation_token: CancellationToken,
}

impl Sv2CpuMinerClientHandler {
    pub fn new(
        user_identity: String,
        nominal_hashrate: f32,
        nominal_hashrate_multiplier: f32,
        n_extended_channels: u8,
        n_standard_channels: u8,
        single_submit: bool,
        cpu_usage_percent: u64,
        event_injector: async_channel::Sender<Sv2ClientEvent<'static>>,
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
            extended_channels: Arc::new(RwLock::new(HashMap::with_capacity(
                n_extended_channels as usize,
            ))),
            standard_channels: Arc::new(RwLock::new(HashMap::with_capacity(
                n_standard_channels as usize,
            ))),
            event_injector,
            cancellation_token,
        }
    }
}

impl Sv2MiningClientHandler for Sv2CpuMinerClientHandler {
    async fn start(&mut self) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        let mut requests = Vec::new();

        let nominal_hashrate_per_channel = (self.nominal_hashrate
            * self.nominal_hashrate_multiplier)
            / (self.n_standard_channels + self.n_extended_channels) as f32;

        for i in 0..self.n_standard_channels {
            info!(
                "Sending OpenStandardMiningChannel with nominal hashrate: {} H/s",
                format_number_with_underscores(nominal_hashrate_per_channel as u64)
            );
            requests.push(Sv2ClientEvent::MiningTrigger(
                MiningClientTrigger::OpenStandardMiningChannel(
                    i as u32,
                    self.user_identity.clone(),
                    nominal_hashrate_per_channel,
                    vec![0xFF_u8; 32], // allow maximum possible target
                ),
            ));
        }

        for i in 0..self.n_extended_channels {
            info!(
                "Sending OpenExtendedMiningChannel with nominal hashrate: {} H/s",
                format_number_with_underscores(nominal_hashrate_per_channel as u64)
            );
            requests.push(Sv2ClientEvent::MiningTrigger(
                MiningClientTrigger::OpenExtendedMiningChannel(
                    (i + self.n_standard_channels) as u32,
                    self.user_identity.clone(),
                    nominal_hashrate_per_channel,
                    vec![0xFF_u8; 32], // allow maximum possible target
                    0, // no extranonce rolling to avoid merkle root calculation overhead
                ),
            ));
        }
        Ok(Sv2ClientOutcome::TriggerNewEvent(Box::new(
            Sv2ClientEvent::MultipleEvents(Box::new(requests)),
        )))
    }

    async fn handle_open_standard_mining_channel_success(
        &mut self,
        open_standard_mining_channel_success: OpenStandardMiningChannelSuccess<'static>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!(
            "Received OpenStandardMiningChannel.Success: {}",
            open_standard_mining_channel_success
        );

        let standard_channel = StandardChannel::new(
            open_standard_mining_channel_success.channel_id,
            self.user_identity.clone(),
            open_standard_mining_channel_success
                .extranonce_prefix
                .to_vec(),
            open_standard_mining_channel_success.target.into(),
            self.nominal_hashrate / (self.n_standard_channels + self.n_extended_channels) as f32,
        );

        let mut standard_channels = self.standard_channels.write().await;

        standard_channels.insert(
            open_standard_mining_channel_success.channel_id,
            Arc::new(RwLock::new(StandardMiner::new(
                standard_channel.clone(),
                self.cpu_usage_percent,
                self.single_submit,
                self.event_injector.clone(),
                self.cancellation_token.clone(),
            ))),
        );

        debug!("Created new Standard Channel: {:?}", standard_channel);

        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_open_extended_mining_channel_success(
        &mut self,
        open_extended_mining_channel_success: OpenExtendedMiningChannelSuccess<'static>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!(
            "Received OpenExtendedMiningChannel.Success: {}",
            open_extended_mining_channel_success
        );

        let extended_channel = ExtendedChannel::new(
            open_extended_mining_channel_success.channel_id,
            self.user_identity.clone(),
            open_extended_mining_channel_success
                .extranonce_prefix
                .to_vec(),
            open_extended_mining_channel_success.target.into(),
            self.nominal_hashrate / (self.n_standard_channels + self.n_extended_channels) as f32,
            true,
            open_extended_mining_channel_success.extranonce_size,
        );

        let mut extended_channels = self.extended_channels.write().await;

        extended_channels.insert(
            open_extended_mining_channel_success.channel_id,
            Arc::new(RwLock::new(ExtendedMiner::new(
                extended_channel.clone(),
                self.cpu_usage_percent,
                self.single_submit,
                self.event_injector.clone(),
                self.cancellation_token.clone(),
            ))),
        );

        debug!("Created new Extended Channel: {:?}", extended_channel);

        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_open_mining_channel_error(
        &mut self,
        open_standard_mining_channel_error: OpenMiningChannelError<'static>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!(
            "Received OpenMiningChannel.Error: {}",
            open_standard_mining_channel_error
        );
        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_update_channel_error(
        &mut self,
        update_channel_error: UpdateChannelError<'static>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("Received UpdateChannel.Error: {}", update_channel_error);
        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_close_channel(
        &mut self,
        close_channel: CloseChannel<'static>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("Received CloseChannel: {}", close_channel);

        let mut standard_channels = self.standard_channels.write().await;
        let mut extended_channels = self.extended_channels.write().await;

        let has_standard_channel = standard_channels.contains_key(&close_channel.channel_id);
        let has_extended_channel = extended_channels.contains_key(&close_channel.channel_id);

        if has_standard_channel {
            standard_channels.remove(&close_channel.channel_id);
            info!(
                "Removed Standard Channel with ID: {}",
                close_channel.channel_id
            );
        }

        if has_extended_channel {
            extended_channels.remove(&close_channel.channel_id);
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

        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_set_extranonce_prefix(
        &mut self,
        set_extranonce_prefix: SetExtranoncePrefix<'static>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("received SetExtranoncePrefix: {}", set_extranonce_prefix);

        let standard_channels = self.standard_channels.read().await;
        let extended_channels = self.extended_channels.read().await;

        let has_standard_channel =
            standard_channels.contains_key(&set_extranonce_prefix.channel_id);
        let has_extended_channel =
            extended_channels.contains_key(&set_extranonce_prefix.channel_id);

        if has_standard_channel {
            let mut standard_channel = standard_channels
                .get(&set_extranonce_prefix.channel_id)
                .expect("channel id must exist")
                .write()
                .await;

            match standard_channel
                .set_extranonce_prefix(set_extranonce_prefix.extranonce_prefix.to_vec())
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
            let mut extended_channel = extended_channels
                .get(&set_extranonce_prefix.channel_id)
                .expect("channel id must exist")
                .write()
                .await;

            match extended_channel
                .set_extranonce_prefix(set_extranonce_prefix.extranonce_prefix.to_vec())
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

        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_submit_shares_success(
        &mut self,
        submit_shares_success: SubmitSharesSuccess,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("received SubmitShares.Success: {}", submit_shares_success);
        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_submit_shares_error(
        &mut self,
        submit_shares_error: SubmitSharesError<'_>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("received SubmitShares.Error: {}", submit_shares_error);
        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_new_mining_job(
        &mut self,
        new_mining_job: NewMiningJob<'_>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("Received NewMiningJob: {}", new_mining_job);

        let standard_channels = self.standard_channels.read().await;

        let has_standard_channel = standard_channels.contains_key(&new_mining_job.channel_id);

        if !has_standard_channel {
            error!(
                "Standard Channel ID: {} not found. Ignoring NewMiningJob.",
                new_mining_job.channel_id
            );
        } else {
            let mut standard_channel = standard_channels
                .get(&new_mining_job.channel_id)
                .expect("channel id must exist")
                .write()
                .await;
            standard_channel
                .on_new_mining_job(new_mining_job.clone().into_static())
                .await;
            info!(
                "NewMiningJob processed: Standard Channel ID: {}, Job ID: {}",
                new_mining_job.channel_id, new_mining_job.job_id
            );
        }

        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_new_extended_mining_job(
        &mut self,
        new_extended_mining_job: NewExtendedMiningJob<'_>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("Received NewExtendedMiningJob: {}", new_extended_mining_job);

        let extended_channels = self.extended_channels.read().await;

        let has_extended_channel =
            extended_channels.contains_key(&new_extended_mining_job.channel_id);

        if !has_extended_channel {
            error!(
                "Extended Channel ID: {} not found. Ignoring NewExtendedMiningJob.",
                new_extended_mining_job.channel_id
            );
        } else {
            let mut extended_channel = extended_channels
                .get(&new_extended_mining_job.channel_id)
                .expect("channel id must exist")
                .write()
                .await;
            extended_channel
                .on_new_extended_mining_job(new_extended_mining_job.clone().into_static())
                .await;
            info!(
                "NewExtendedMiningJob processed: Extended Channel ID: {:?}, Job ID: {:?}",
                new_extended_mining_job.channel_id, new_extended_mining_job.job_id
            );
        }
        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_set_new_prev_hash(
        &mut self,
        set_new_prev_hash: SetNewPrevHash<'_>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("Received SetNewPrevHash: {}", set_new_prev_hash);

        let standard_channels = self.standard_channels.read().await;
        let extended_channels = self.extended_channels.read().await;

        let has_standard_channel = standard_channels.contains_key(&set_new_prev_hash.channel_id);
        let has_extended_channel = extended_channels.contains_key(&set_new_prev_hash.channel_id);

        if !has_standard_channel && !has_extended_channel {
            error!(
                "Channel with ID: {} not found, ignoring SetNewPrevHash.",
                set_new_prev_hash.channel_id
            );
        }

        if has_standard_channel {
            let mut standard_channel = standard_channels
                .get(&set_new_prev_hash.channel_id)
                .expect("channel id must exist")
                .write()
                .await;

            match standard_channel
                .on_set_new_prev_hash(set_new_prev_hash.clone().into_static())
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
            let mut extended_channel = extended_channels
                .get(&set_new_prev_hash.channel_id)
                .expect("channel id must exist")
                .write()
                .await;

            match extended_channel
                .on_set_new_prev_hash(set_new_prev_hash.clone().into_static())
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

        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_set_custom_mining_job_success(
        &mut self,
        _set_custom_mining_job_success: SetCustomMiningJobSuccess,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        error!("Received unexpected SetCustomMiningJob.Success");
        Err(Sv2ClientEventError::UnsupportedMessage)
    }

    async fn handle_set_custom_mining_job_error(
        &mut self,
        _set_custom_mining_job_error: SetCustomMiningJobError<'_>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        error!("Received unexpected SetCustomMiningJob.Error");
        Err(Sv2ClientEventError::UnsupportedMessage)
    }

    async fn handle_set_target(
        &mut self,
        set_target: SetTarget<'_>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        info!("Received SetTarget: {}", set_target);

        let standard_channels = self.standard_channels.read().await;
        let extended_channels = self.extended_channels.read().await;

        let has_standard_channel = standard_channels.contains_key(&set_target.channel_id);
        let has_extended_channel = extended_channels.contains_key(&set_target.channel_id);

        if !has_standard_channel && !has_extended_channel {
            error!(
                "Channel with ID: {} not found, ignoring SetTarget.",
                set_target.channel_id
            );
        }

        if has_standard_channel {
            let mut standard_channel = standard_channels
                .get(&set_target.channel_id)
                .expect("channel id must exist")
                .write()
                .await;

            standard_channel
                .set_target(set_target.maximum_target.clone().into())
                .await;
            info!(
                "SetTarget processed: Standard Channel ID: {}",
                set_target.channel_id
            );
        }

        if has_extended_channel {
            let mut extended_channel = extended_channels
                .get(&set_target.channel_id)
                .expect("channel id must exist")
                .write()
                .await;

            extended_channel
                .set_target(set_target.maximum_target.into())
                .await;
            info!(
                "SetTarget processed: Extended Channel ID: {}",
                set_target.channel_id
            );
        }

        Ok(Sv2ClientOutcome::Ok)
    }

    async fn handle_set_group_channel(
        &mut self,
        _set_group_channel: SetGroupChannel<'_>,
    ) -> Result<Sv2ClientOutcome<'static>, Sv2ClientEventError> {
        error!("Received unexpected SetGroupChannel");
        Err(Sv2ClientEventError::UnsupportedMessage)
    }
}
