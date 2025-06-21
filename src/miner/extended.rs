use tower_stratum::client::service::request::RequestToSv2Client;
use tower_stratum::roles_logic_sv2::channels::client::error::ExtendedChannelError;
use tower_stratum::roles_logic_sv2::channels::client::extended::ExtendedChannel;
use tower_stratum::roles_logic_sv2::mining_sv2::{
    NewExtendedMiningJob, SetNewPrevHash, SubmitSharesExtended, Target,
};
use tower_stratum::roles_logic_sv2::{
    parsers::Mining,
    utils::{merkle_root_from_path, u256_to_block_hash},
};

use crate::config::CPU_THROTTLE_WINDOW_MS;

use bitcoin::{
    CompactTarget,
    blockdata::block::{Header, Version},
    hashes::sha256d::Hash,
};

use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

pub struct ExtendedMiner {
    extended_channel: Arc<RwLock<ExtendedChannel<'static>>>,
    request_injector: async_channel::Sender<RequestToSv2Client<'static>>,
    global_cancellation_token: CancellationToken,
    miner_cancellation_token: CancellationToken,
    single_submit_cancellation_token: Option<CancellationToken>,
    cpu_usage_percent: u64,
}

impl ExtendedMiner {
    pub fn new(
        extended_channel: ExtendedChannel<'static>,
        cpu_usage_percent: u64,
        single_submit: bool,
        request_injector: async_channel::Sender<RequestToSv2Client<'static>>,
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
            request_injector,
            global_cancellation_token,
            miner_cancellation_token,
            single_submit_cancellation_token,
            cpu_usage_percent,
        }
    }

    pub async fn set_extranonce_prefix(
        &mut self,
        extranonce_prefix: Vec<u8>,
    ) -> Result<(), ExtendedChannelError> {
        self.extended_channel
            .write()
            .await
            .set_extranonce_prefix(extranonce_prefix)?;
        Ok(())
    }

    pub async fn on_new_extended_mining_job(
        &mut self,
        new_extended_mining_job: NewExtendedMiningJob<'static>,
    ) {
        let mut extended_channel = self.extended_channel.write().await;
        extended_channel.on_new_extended_mining_job(new_extended_mining_job.clone());

        // this is a non-future job
        // we should start mining immediately
        if let Some(_min_ntime) = new_extended_mining_job.min_ntime.into_inner() {
            if !self.miner_cancellation_token.is_cancelled() {
                // trigger miner cancellation token to kill task of past job
                self.miner_cancellation_token.cancel();
            }
            self.miner_cancellation_token = CancellationToken::new();

            let request_injector = self.request_injector.clone();
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
    }

    pub async fn on_set_new_prev_hash(
        &mut self,
        set_new_prev_hash: SetNewPrevHash<'static>,
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
        let request_injector = self.request_injector.clone();
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

    pub async fn set_target(&mut self, target: Target) {
        let mut extended_channel = self.extended_channel.write().await;
        extended_channel.set_target(target);
    }
}

async fn mine_job(
    extended_channel: Arc<RwLock<ExtendedChannel<'static>>>,
    request_injector: async_channel::Sender<RequestToSv2Client<'static>>,
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
    let (active_job, extranonce_prefix) = extended_channel_guard
        .get_active_job()
        .expect("channel must have active job")
        .clone();
    let channel_target = extended_channel_guard.get_target().clone();
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

    drop(extended_channel_guard);

    let mut nonce = 0;
    let mut ntime = active_job
        .min_ntime
        .into_inner()
        .expect("only active jobs allowed");

    // Time-based throttling: work for cpu_usage_percent ms, then sleep for (100-cpu_usage_percent)ms in CPU_THROTTLE_WINDOW_MS windows  
    let work_duration_ms = cpu_usage_percent;
    let sleep_duration_ms = CPU_THROTTLE_WINDOW_MS - cpu_usage_percent;
    let mut window_start = std::time::Instant::now();

    // avoid rolling extranonce to save CPU hashpower
    // merkle root calculation would introduce overhead
    let extranonce = vec![0; 32 - extranonce_prefix.len()];
    let full_extranonce = [extranonce_prefix.clone(), extranonce.clone()].concat();
    let merkle_root: [u8; 32] = merkle_root_from_path(
        active_job.coinbase_tx_prefix.inner_as_ref(),
        active_job.coinbase_tx_suffix.inner_as_ref(),
        &full_extranonce,
        &active_job.merkle_path.inner_as_ref(),
    )
    .expect("merkle root must be valid")
    .try_into()
    .expect("merkle root must be 32 bytes");

    loop {
        tokio::select! {
            _ = global_cancellation_token.cancelled() => {
                debug!("miner task cancelled... channel id: {} job id: {}", channel_id, active_job.job_id);
                break;
            }
            _ = miner_cancellation_token.cancelled() => {
                debug!("miner task cancelled... channel id: {} job id: {}", channel_id, active_job.job_id);
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
                    version: Version::from_consensus(active_job.version as i32),
                    prev_blockhash: prevhash,
                    merkle_root: (*Hash::from_bytes_ref(
                        &merkle_root
                    ))
                    .into(),
                    time: ntime,
                    bits: CompactTarget::from_consensus(nbits),
                    nonce,
                };

                // mine the header
                let hash = header.block_hash();

                // convert the header hash to a target type for easy comparison
                let raw_hash: [u8; 32] = *hash.to_raw_hash().as_ref();
                let hash_as_target: Target = raw_hash.into();

                // is share valid?
                if hash_as_target <= channel_target {
                    // log share on channel state
                    let mut extended_channel_guard = extended_channel.write().await;

                    let share_accounting = extended_channel_guard.get_share_accounting();
                    let sequence_number = share_accounting.get_last_share_sequence_number() + 1;

                    let share = SubmitSharesExtended {
                        channel_id: channel_id,
                        sequence_number,
                        job_id: active_job.job_id,
                        nonce,
                        ntime,
                        version: active_job.version,
                        extranonce: extranonce.clone().try_into().expect("extranonce must be serializable"),
                    };

                    let _ = extended_channel_guard.validate_share(share.clone());
                    drop(extended_channel_guard);

                    match request_injector.send(RequestToSv2Client::SendMessageToMiningServer(Box::new(Mining::SubmitSharesExtended(share.clone())))).await {
                        Ok(_) => {
                            info!("Submitting share: {:?}", share);
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
