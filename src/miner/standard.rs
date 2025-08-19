use sv2_services::client::service::event::Sv2ClientEvent;
use sv2_services::roles_logic_sv2::channels::client::error::StandardChannelError;
use sv2_services::roles_logic_sv2::channels::client::standard::StandardChannel;
use sv2_services::roles_logic_sv2::mining_sv2::{
    NewMiningJob, SetNewPrevHash, SubmitSharesStandard, Target,
};
use sv2_services::roles_logic_sv2::{parsers::Mining, utils::u256_to_block_hash};

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

pub struct StandardMiner {
    standard_channel: Arc<RwLock<StandardChannel<'static>>>,
    request_injector: async_channel::Sender<Sv2ClientEvent<'static>>,
    global_cancellation_token: CancellationToken,
    miner_cancellation_token: CancellationToken,
    single_submit_cancellation_token: Option<CancellationToken>,
    cpu_usage_percent: u64,
}

impl StandardMiner {
    pub fn new(
        standard_channel: StandardChannel<'static>,
        cpu_usage_percent: u64,
        single_submit: bool,
        request_injector: async_channel::Sender<Sv2ClientEvent<'static>>,
        global_cancellation_token: CancellationToken,
    ) -> Self {
        let miner_cancellation_token = CancellationToken::new();
        let single_submit_cancellation_token = if single_submit {
            Some(CancellationToken::new())
        } else {
            None
        };
        Self {
            standard_channel: Arc::new(RwLock::new(standard_channel)),
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
    ) -> Result<(), StandardChannelError> {
        self.standard_channel
            .write()
            .await
            .set_extranonce_prefix(extranonce_prefix)?;
        Ok(())
    }

    pub async fn on_new_mining_job(&mut self, new_mining_job: NewMiningJob<'static>) {
        let mut standard_channel = self.standard_channel.write().await;
        standard_channel.on_new_mining_job(new_mining_job.clone());

        // this is a non-future job
        // we should start mining immediately
        if let Some(_min_ntime) = new_mining_job.min_ntime.into_inner() {
            if !self.miner_cancellation_token.is_cancelled() {
                // trigger miner cancellation token to kill task of past job
                self.miner_cancellation_token.cancel();
            }
            self.miner_cancellation_token = CancellationToken::new();

            let request_injector = self.request_injector.clone();
            let global_cancellation_token = self.global_cancellation_token.clone();
            let miner_cancellation_token = self.miner_cancellation_token.clone();
            let standard_channel = self.standard_channel.clone();
            let cpu_usage_percent = self.cpu_usage_percent;
            let single_submit_cancellation_token = self.single_submit_cancellation_token.clone();

            tokio::spawn(async move {
                mine_job(
                    standard_channel,
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
    ) -> Result<(), StandardChannelError> {
        let mut standard_channel = self.standard_channel.write().await;
        standard_channel.on_set_new_prev_hash(set_new_prev_hash.clone())?;
        drop(standard_channel);

        if !self.miner_cancellation_token.is_cancelled() {
            // trigger miner cancellation token to kill task of past job
            self.miner_cancellation_token.cancel();
        }
        self.miner_cancellation_token = CancellationToken::new();

        // Extract needed values from self before spawning
        let request_injector = self.request_injector.clone();
        let global_cancellation_token = self.global_cancellation_token.clone();
        let miner_cancellation_token = self.miner_cancellation_token.clone();
        let standard_channel = self.standard_channel.clone();
        let cpu_usage_percent = self.cpu_usage_percent;
        let single_submit_cancellation_token = self.single_submit_cancellation_token.clone();

        tokio::spawn(async move {
            mine_job(
                standard_channel,
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
        let mut standard_channel = self.standard_channel.write().await;
        standard_channel.set_target(target);
    }
}

async fn mine_job(
    standard_channel: Arc<RwLock<StandardChannel<'static>>>,
    request_injector: async_channel::Sender<Sv2ClientEvent<'static>>,
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

    let standard_channel_guard = standard_channel.read().await;
    let channel_id = standard_channel_guard.get_channel_id();
    let active_job = standard_channel_guard
        .get_active_job()
        .expect("channel must have active job")
        .clone();
    let channel_target = standard_channel_guard.get_target().clone();
    let nbits = standard_channel_guard
        .get_chain_tip()
        .expect("channel must have chain tip")
        .nbits();
    let prevhash = u256_to_block_hash(
        standard_channel_guard
            .get_chain_tip()
            .expect("channel must have chain tip")
            .prev_hash(),
    );

    drop(standard_channel_guard);

    let mut nonce = 0;
    let mut ntime = active_job
        .min_ntime
        .into_inner()
        .expect("only active jobs allowed");

    // Time-based throttling: work for cpu_usage_percent ms, then sleep for (100-cpu_usage_percent)ms in CPU_THROTTLE_WINDOW_MS windows
    let work_duration_ms = cpu_usage_percent;
    let sleep_duration_ms = CPU_THROTTLE_WINDOW_MS - cpu_usage_percent;
    let mut window_start = std::time::Instant::now();

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
                        active_job.merkle_root
                            .inner_as_ref()
                            .try_into()
                            .expect("merkle_root should be 32 bytes"),
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
                    let mut standard_channel_guard = standard_channel.write().await;

                    let share_accounting = standard_channel_guard.get_share_accounting();
                    let sequence_number = share_accounting.get_last_share_sequence_number() + 1;

                    let share = SubmitSharesStandard {
                        channel_id: channel_id,
                        sequence_number,
                        job_id: active_job.job_id,
                        nonce,
                        ntime,
                        version: active_job.version,
                    };


                    let _ = standard_channel_guard.validate_share(share.clone());
                    drop(standard_channel_guard);

                    match request_injector.send(Sv2ClientEvent::SendMessageToMiningServer(Box::new(Mining::SubmitSharesStandard(share.clone())))).await {
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
