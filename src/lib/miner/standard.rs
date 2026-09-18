//! Mining on a standard channel. Jobs arrive as `NewMiningJob`, or as a group's
//! `NewExtendedMiningJob` from which the channel derives its own merkle root.

use crate::error::Sv2CpuMinerError;
use stratum_apps::stratum_core::bitcoin::{
    CompactTarget, Target,
    blockdata::block::{Header, Version},
    hashes::sha256d::Hash,
};
use stratum_apps::stratum_core::channels_sv2::client::share_accounting::ShareValidationResult;
use stratum_apps::stratum_core::channels_sv2::client::standard::StandardChannel;
use stratum_apps::stratum_core::channels_sv2::extranonce_manager::ExtranoncePrefix;
use stratum_apps::stratum_core::channels_sv2::target::u256_to_block_hash;
use stratum_apps::stratum_core::mining_sv2::{
    NewExtendedMiningJobOwned, NewMiningJobOwned, SetNewPrevHashOwned, SubmitSharesStandardOwned,
};
use stratum_apps::stratum_core::parsers_sv2::MiningOwned;
use stratum_apps::sync::SharedRw;
use stratum_apps::utils::types::{Message, OutboundFrame};

use super::CPU_THROTTLE_WINDOW_MS;

use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// A standard channel and the task hashing on its active job. A new active job or prev hash
/// replaces the task.
pub struct StandardChannelMiner {
    standard_channel: SharedRw<StandardChannel>,
    upstream_sender: async_channel::Sender<OutboundFrame>,
    global_cancellation_token: CancellationToken,
    miner_cancellation_token: CancellationToken,
    single_submit_cancellation_token: Option<CancellationToken>,
    cpu_usage_percent: u64,
}

impl StandardChannelMiner {
    pub fn new(
        standard_channel: StandardChannel,
        cpu_usage_percent: u64,
        single_submit: bool,
        upstream_sender: async_channel::Sender<OutboundFrame>,
        global_cancellation_token: CancellationToken,
    ) -> Self {
        let miner_cancellation_token = CancellationToken::new();
        let single_submit_cancellation_token = if single_submit {
            Some(CancellationToken::new())
        } else {
            None
        };
        Self {
            standard_channel: SharedRw::new(standard_channel),
            upstream_sender,
            global_cancellation_token,
            miner_cancellation_token,
            single_submit_cancellation_token,
            cpu_usage_percent,
        }
    }

    /// Full extranonce size of this channel, which every member of a group must share.
    pub fn full_extranonce_size(&self) -> Result<usize, Sv2CpuMinerError> {
        Ok(self
            .standard_channel
            .read(|channel| channel.get_extranonce_prefix().len())?)
    }

    #[cfg(test)]
    pub fn future_job_ids(&self) -> Result<Vec<u32>, Sv2CpuMinerError> {
        Ok(self.standard_channel.read(|channel| {
            channel
                .get_future_jobs()
                .map(|(job_id, _)| *job_id)
                .collect()
        })?)
    }

    pub fn set_extranonce_prefix(
        &mut self,
        extranonce_prefix: ExtranoncePrefix,
    ) -> Result<(), Sv2CpuMinerError> {
        self.standard_channel
            .write(|channel| channel.set_extranonce_prefix(extranonce_prefix))??;
        Ok(())
    }

    /// Cancels the mining task of the past job (if any) and spawns a fresh one
    /// for the currently active job.
    fn respawn_mining_task(&mut self) {
        if !self.miner_cancellation_token.is_cancelled() {
            // trigger miner cancellation token to kill task of past job
            self.miner_cancellation_token.cancel();
        }
        self.miner_cancellation_token = CancellationToken::new();

        // Extract needed values from self before spawning
        let upstream_sender = self.upstream_sender.clone();
        let global_cancellation_token = self.global_cancellation_token.clone();
        let miner_cancellation_token = self.miner_cancellation_token.clone();
        let standard_channel = self.standard_channel.clone();
        let cpu_usage_percent = self.cpu_usage_percent;
        let single_submit_cancellation_token = self.single_submit_cancellation_token.clone();

        tokio::spawn(async move {
            mine_job(
                standard_channel,
                upstream_sender,
                global_cancellation_token,
                miner_cancellation_token,
                single_submit_cancellation_token,
                cpu_usage_percent,
            )
            .await;
        });
    }

    /// Stores the job. If it is already active, that is, it carries a `min_ntime`, mining
    /// restarts on it at once.
    pub fn on_new_mining_job(
        &mut self,
        new_mining_job: NewMiningJobOwned,
    ) -> Result<(), Sv2CpuMinerError> {
        self.standard_channel
            .write(|channel| channel.on_new_mining_job(new_mining_job.clone()))??;

        // this is a non-future job
        // we should start mining immediately
        if let Some(_min_ntime) = new_mining_job.min_ntime.into_inner() {
            self.respawn_mining_task();
        }

        Ok(())
    }

    /// Handles a NewExtendedMiningJob addressed to the group channel this standard
    /// channel belongs to. The channel state converts it into a per-channel standard
    /// job using its own extranonce prefix.
    pub fn on_group_channel_job(
        &mut self,
        new_extended_mining_job: NewExtendedMiningJobOwned,
    ) -> Result<(), Sv2CpuMinerError> {
        self.standard_channel
            .write(|channel| channel.on_new_group_channel_job(new_extended_mining_job.clone()))??;

        // this is a non-future job
        // we should start mining immediately
        if let Some(_min_ntime) = new_extended_mining_job.min_ntime.into_inner() {
            self.respawn_mining_task();
        }

        Ok(())
    }

    /// Activates the referenced future job and restarts mining on the new chain tip.
    pub fn on_set_new_prev_hash(
        &mut self,
        set_new_prev_hash: SetNewPrevHashOwned,
    ) -> Result<(), Sv2CpuMinerError> {
        self.standard_channel
            .write(|channel| channel.on_set_new_prev_hash(set_new_prev_hash.clone()))??;

        self.respawn_mining_task();

        Ok(())
    }

    pub fn set_target(&mut self, target: Target) -> Result<(), Sv2CpuMinerError> {
        self.standard_channel
            .write(|channel| channel.set_target(target))??;
        Ok(())
    }
}

/// Hashes the channel's active job, rolling nonce and ntime, until cancelled. Works for
/// `cpu_usage_percent` of every throttle window and sleeps for the rest.
async fn mine_job(
    standard_channel: SharedRw<StandardChannel>,
    upstream_sender: async_channel::Sender<OutboundFrame>,
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

    let Ok((channel_id, active_job, channel_target, nbits, prevhash)) =
        standard_channel.read(|channel| {
            let chain_tip = channel
                .get_chain_tip()
                .expect("channel must have chain tip");
            (
                channel.get_channel_id(),
                channel
                    .get_active_job()
                    .expect("channel must have active job")
                    .clone(),
                *channel.get_target(),
                chain_tip.nbits(),
                u256_to_block_hash(chain_tip.prev_hash()),
            )
        })
    else {
        error!("Channel lock poisoned, stopping the miner task");
        return;
    };

    let job_id = active_job.job_message.job_id;
    let version = active_job.job_message.version;
    let merkle_root: [u8; 32] = active_job.job_message.merkle_root.to_array();

    let mut nonce = 0;
    let mut ntime = active_job
        .job_message
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
                    let Ok((share, share_validation_result)) = standard_channel.write(|channel| {
                        let sequence_number = channel
                            .get_share_accounting()
                            .get_last_share_sequence_number()
                            + 1;
                        let share = SubmitSharesStandardOwned {
                            channel_id,
                            sequence_number,
                            job_id,
                            nonce,
                            ntime,
                            version,
                        };
                        let share_validation_result = channel.validate_share(share.clone());
                        (share, share_validation_result)
                    }) else {
                        error!("Channel lock poisoned, stopping the miner task");
                        return;
                    };

                    // the channel re-derives the share from its own job state and decides
                    // whether it is worth submitting; only validated shares advance the
                    // sequence number
                    match share_validation_result {
                        Err(e) => {
                            warn!("Not submitting share rejected by channel validation: {:?}, share: {}", e, share);
                        }
                        Ok(result) => {
                            if let ShareValidationResult::BlockFound(hash) = result {
                                info!("Block found! hash: {}, share: {}", hash, share);
                            }

                            let submit = async {
                                let frame = OutboundFrame::from_message(Message::Mining(
                                    MiningOwned::SubmitSharesStandard(share.clone()),
                                ))?;
                                upstream_sender.send(frame).await?;
                                Ok::<(), Sv2CpuMinerError>(())
                            };

                            match submit.await {
                                Ok(()) => {
                                    info!("Submitting share: {}", share);
                                    if let Some(ref single_submit_cancellation_token) = single_submit_cancellation_token {
                                        info!("Single submit enabled, cancelling miner task");
                                        single_submit_cancellation_token.cancel();
                                        break;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to submit share: {}", e);
                                }
                            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use stratum_apps::stratum_core::binary_sv2::Sv2OptionOwned;
    use stratum_apps::stratum_core::channels_sv2::client::standard::StandardChannel;
    use tokio::time::timeout;

    fn channel(target: Target) -> StandardChannel {
        StandardChannel::new(
            1,
            "user".to_string(),
            ExtranoncePrefix::from_wire(vec![0; 32]).unwrap(),
            target,
            1.0,
            None,
        )
        .unwrap()
    }

    /// A miner whose job 1 is active under `channel_target`, with the mining task spawned but
    /// not yet started: a current-thread runtime only runs it once the test awaits.
    fn miner_with_active_job(
        channel_target: Target,
    ) -> (StandardChannelMiner, async_channel::Receiver<OutboundFrame>) {
        let (upstream_sender, receiver) = async_channel::unbounded();
        let mut miner = StandardChannelMiner::new(
            channel(channel_target),
            100,
            false,
            upstream_sender,
            CancellationToken::new(),
        );
        miner
            .on_new_mining_job(NewMiningJobOwned {
                channel_id: 1,
                job_id: 1,
                min_ntime: Sv2OptionOwned::new(None),
                version: 536870912,
                merkle_root: [0u8; 32].into(),
            })
            .unwrap();
        miner
            .on_set_new_prev_hash(SetNewPrevHashOwned {
                channel_id: 1,
                job_id: 1,
                prev_hash: [0u8; 32].into(),
                nbits: 453040064,
                min_ntime: 1745596970,
            })
            .unwrap();
        (miner, receiver)
    }

    #[tokio::test]
    async fn shares_the_channel_accepts_are_submitted() {
        let (miner, receiver) = miner_with_active_job(Target::from_le_bytes([0xff; 32]));

        assert!(
            timeout(Duration::from_secs(5), receiver.recv())
                .await
                .is_ok(),
            "no share reached the upstream sender"
        );
        miner.miner_cancellation_token.cancel();
    }

    /// The miner's pre-check uses the channel target, relaxed here after the job became active,
    /// so it finds "shares" at once; the job keeps its unreachable target, so the channel rejects
    /// every one of them and none may be submitted.
    #[tokio::test]
    async fn shares_the_channel_rejects_are_not_submitted() {
        let mut unreachable = [0u8; 32];
        unreachable[0] = 1;
        let (mut miner, receiver) = miner_with_active_job(Target::from_le_bytes(unreachable));
        miner.set_target(Target::from_le_bytes([0xff; 32])).unwrap();

        assert!(
            timeout(Duration::from_millis(300), receiver.recv())
                .await
                .is_err(),
            "a share the channel rejected was submitted"
        );
        miner.miner_cancellation_token.cancel();
    }

    #[test]
    fn poisoned_lock_is_an_error_not_a_panic() {
        let (upstream_sender, _receiver) = async_channel::unbounded();
        let mut miner = StandardChannelMiner::new(
            channel(Target::from_le_bytes([0xff; 32])),
            100,
            false,
            upstream_sender,
            CancellationToken::new(),
        );

        let poison = catch_unwind(AssertUnwindSafe(|| {
            let _ = miner
                .standard_channel
                .write(|_channel| -> () { panic!("poison") });
        }));
        assert!(poison.is_err());

        assert!(matches!(
            miner.set_target(Target::from_le_bytes([1; 32])),
            Err(Sv2CpuMinerError::PoisonLock)
        ));
        assert!(matches!(
            miner.full_extranonce_size(),
            Err(Sv2CpuMinerError::PoisonLock)
        ));
    }
}
