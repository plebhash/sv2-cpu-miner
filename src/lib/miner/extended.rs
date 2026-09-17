use crate::client::{Message, StdFrame};
use stratum_apps::stratum_core::bitcoin::{
    CompactTarget, Target,
    blockdata::block::{Header, Version},
    hashes::sha256d::Hash,
};
use stratum_apps::stratum_core::channels_sv2::client::error::ExtendedChannelError;
use stratum_apps::stratum_core::channels_sv2::client::extended::ExtendedChannel;
use stratum_apps::stratum_core::channels_sv2::extranonce_manager::ExtranoncePrefix;
use stratum_apps::stratum_core::channels_sv2::merkle_root::merkle_root_from_path;
use stratum_apps::stratum_core::channels_sv2::target::u256_to_block_hash;
use stratum_apps::stratum_core::mining_sv2::{
    NewExtendedMiningJobOwned, SetNewPrevHashOwned, SubmitSharesExtendedOwned,
};
use stratum_apps::stratum_core::parsers_sv2::MiningOwned;

use crate::config::CPU_THROTTLE_WINDOW_MS;

use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

pub struct ExtendedMiner {
    extended_channel: Arc<RwLock<ExtendedChannel>>,
    event_injector: async_channel::Sender<StdFrame>,
    global_cancellation_token: CancellationToken,
    miner_cancellation_token: CancellationToken,
    single_submit_cancellation_token: Option<CancellationToken>,
    cpu_usage_percent: u64,
}

impl ExtendedMiner {
    pub fn new(
        extended_channel: ExtendedChannel,
        cpu_usage_percent: u64,
        single_submit: bool,
        event_injector: async_channel::Sender<StdFrame>,
        global_cancellation_token: CancellationToken,
    ) -> Self {
        let miner_cancellation_token = CancellationToken::new();
        let single_submit_cancellation_token = if single_submit {
            Some(CancellationToken::new())
        } else {
            None
        };
        Self {
            extended_channel: Arc::new(RwLock::new(extended_channel)),
            event_injector,
            global_cancellation_token,
            miner_cancellation_token,
            single_submit_cancellation_token,
            cpu_usage_percent,
        }
    }

    /// Full extranonce size of this channel, which every member of a group must share.
    pub async fn full_extranonce_size(&self) -> usize {
        self.extended_channel
            .read()
            .await
            .get_full_extranonce_size()
    }

    pub async fn set_extranonce_prefix(
        &mut self,
        extranonce_prefix: ExtranoncePrefix,
    ) -> Result<(), ExtendedChannelError> {
        self.extended_channel
            .write()
            .await
            .set_extranonce_prefix(extranonce_prefix)?;
        Ok(())
    }

    pub async fn on_new_extended_mining_job(
        &mut self,
        new_extended_mining_job: NewExtendedMiningJobOwned,
    ) -> Result<(), ExtendedChannelError> {
        let mut extended_channel = self.extended_channel.write().await;
        extended_channel.on_new_extended_mining_job(new_extended_mining_job.clone())?;
        drop(extended_channel);

        // this is a non-future job
        // we should start mining immediately
        if let Some(_min_ntime) = new_extended_mining_job.min_ntime.into_inner() {
            if !self.miner_cancellation_token.is_cancelled() {
                // trigger miner cancellation token to kill task of past job
                self.miner_cancellation_token.cancel();
            }
            self.miner_cancellation_token = CancellationToken::new();

            let request_injector = self.event_injector.clone();
            let global_cancellation_token = self.global_cancellation_token.clone();
            let miner_cancellation_token = self.miner_cancellation_token.clone();
            let extended_channel = self.extended_channel.clone();
            let cpu_usage_percent = self.cpu_usage_percent;
            let single_submit_cancellation_token = self.single_submit_cancellation_token.clone();

            tokio::spawn(async move {
                mine_job(
                    extended_channel,
                    request_injector,
                    global_cancellation_token,
                    miner_cancellation_token,
                    single_submit_cancellation_token,
                    cpu_usage_percent,
                )
                .await;
            });
        }

        Ok(())
    }

    pub async fn on_set_new_prev_hash(
        &mut self,
        set_new_prev_hash: SetNewPrevHashOwned,
    ) -> Result<(), ExtendedChannelError> {
        let mut extended_channel = self.extended_channel.write().await;
        extended_channel.on_set_new_prev_hash(set_new_prev_hash.clone())?;
        drop(extended_channel);

        if !self.miner_cancellation_token.is_cancelled() {
            // trigger miner cancellation token to kill task of past job
            self.miner_cancellation_token.cancel();
        }
        self.miner_cancellation_token = CancellationToken::new();

        // Extract needed values from self before spawning
        let request_injector = self.event_injector.clone();
        let global_cancellation_token = self.global_cancellation_token.clone();
        let miner_cancellation_token = self.miner_cancellation_token.clone();
        let extended_channel = self.extended_channel.clone();
        let cpu_usage_percent = self.cpu_usage_percent;
        let single_submit_cancellation_token = self.single_submit_cancellation_token.clone();

        tokio::spawn(async move {
            mine_job(
                extended_channel,
                request_injector,
                global_cancellation_token,
                miner_cancellation_token,
                single_submit_cancellation_token,
                cpu_usage_percent,
            )
            .await;
        });

        Ok(())
    }

    pub async fn set_target(&mut self, target: Target) -> Result<(), ExtendedChannelError> {
        let mut extended_channel = self.extended_channel.write().await;
        extended_channel.set_target(target)?;
        Ok(())
    }
}

async fn mine_job(
    extended_channel: Arc<RwLock<ExtendedChannel>>,
    event_injector: async_channel::Sender<StdFrame>,
    global_cancellation_token: CancellationToken,
    miner_cancellation_token: CancellationToken,
    single_submit_cancellation_token: Option<CancellationToken>,
    cpu_usage_percent: u64,
) {
    if let Some(ref single_submit_cancellation_token) = single_submit_cancellation_token {
        if single_submit_cancellation_token.is_cancelled() {
            info!("Single submit enabled, cancelling miner task");
            return;
        }
    }

    let extended_channel_guard = extended_channel.read().await;
    let channel_id = extended_channel_guard.get_channel_id();
    let active_job = extended_channel_guard
        .get_active_job()
        .expect("channel must have active job")
        .clone();
    let channel_target = *extended_channel_guard.get_target();
    let nbits = extended_channel_guard
        .get_chain_tip()
        .expect("channel must have chain tip")
        .nbits();
    let prevhash = u256_to_block_hash(
        extended_channel_guard
            .get_chain_tip()
            .expect("channel must have chain tip")
            .prev_hash(),
    );
    let extranonce_size = extended_channel_guard.get_rollable_extranonce_size() as usize;

    drop(extended_channel_guard);

    let job_id = active_job.job_message.job_id;
    let version = active_job.job_message.version;

    let mut nonce = 0;
    let mut ntime = active_job
        .job_message
        .min_ntime
        .clone()
        .into_inner()
        .expect("only active jobs allowed");

    // Time-based throttling: work for cpu_usage_percent ms, then sleep for (100-cpu_usage_percent)ms in CPU_THROTTLE_WINDOW_MS windows
    let work_duration_ms = cpu_usage_percent;
    let sleep_duration_ms = CPU_THROTTLE_WINDOW_MS - cpu_usage_percent;
    let mut window_start = std::time::Instant::now();

    // avoid rolling extranonce to save CPU hashpower
    // merkle root calculation would introduce overhead
    let extranonce = vec![0; extranonce_size];
    let full_extranonce = [active_job.extranonce_prefix.clone(), extranonce.clone()].concat();
    let merkle_root: [u8; 32] = merkle_root_from_path(
        active_job.job_message.coinbase_tx_prefix.as_bytes(),
        active_job.job_message.coinbase_tx_suffix.as_bytes(),
        &full_extranonce,
        active_job.job_message.merkle_path.as_slice(),
    )
    .expect("merkle root must be valid");

    loop {
        tokio::select! {
            _ = global_cancellation_token.cancelled() => {
                debug!("miner task cancelled... channel id: {} job id: {}", channel_id, job_id);
                break;
            }
            _ = miner_cancellation_token.cancelled() => {
                debug!("miner task cancelled... channel id: {} job id: {}", channel_id, job_id);
                break;
            }
            _ = tokio::task::yield_now() => {
                // Time-based CPU throttling
                if cpu_usage_percent < 100 {
                    let elapsed_in_window = window_start.elapsed().as_millis() as u64;
                    if elapsed_in_window >= work_duration_ms {
                        // Time to sleep for the throttle period
                        tokio::time::sleep(Duration::from_millis(sleep_duration_ms)).await;
                        window_start = std::time::Instant::now(); // Reset window
                    }
                }

                let header = Header {
                    version: Version::from_consensus(version as i32),
                    prev_blockhash: prevhash,
                    merkle_root: (*Hash::from_bytes_ref(&merkle_root)).into(),
                    time: ntime,
                    bits: CompactTarget::from_consensus(nbits),
                    nonce,
                };

                // mine the header
                let hash = header.block_hash();

                // convert the header hash to a target type for easy comparison
                let raw_hash: [u8; 32] = *hash.to_raw_hash().as_ref();
                let hash_as_target = Target::from_le_bytes(raw_hash);

                // is share valid?
                if hash_as_target <= channel_target {
                    // log share on channel state
                    let mut extended_channel_guard = extended_channel.write().await;

                    let share_accounting = extended_channel_guard.get_share_accounting();
                    let sequence_number = share_accounting.get_last_share_sequence_number() + 1;

                    let share = SubmitSharesExtendedOwned {
                        channel_id,
                        sequence_number,
                        job_id,
                        nonce,
                        ntime,
                        version,
                        extranonce: extranonce.clone().try_into().expect("extranonce must be serializable"),
                    };

                    let _ = extended_channel_guard.validate_share(share.clone());
                    drop(extended_channel_guard);

                    let frame: StdFrame = Message::Mining(MiningOwned::SubmitSharesExtended(share.clone()))
                        .try_into()
                        .expect("SubmitSharesExtended must be serializable");

                    match event_injector.send(frame).await {
                        Ok(_) => {
                            info!("Submitting share: {}", share);
                            if let Some(ref single_submit_cancellation_token) = single_submit_cancellation_token {
                                info!("Single submit enabled, cancelling miner task");
                                single_submit_cancellation_token.cancel();
                            }
                        }
                        Err(e) => {
                            error!("Failed to send share: {}", e);
                        }
                    }
                }

                nonce = match nonce.checked_add(1) {
                    Some(nonce) => nonce,
                    None => {
                        ntime = match ntime.checked_add(1) {
                            Some(ntime) => ntime,
                            None => {
                                error!("Both nonce and ntime overflowed");
                                break;
                            }
                        };
                        0
                    }
                };
            }
        }
    }
}
