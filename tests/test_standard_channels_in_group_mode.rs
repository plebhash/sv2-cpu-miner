use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use integration_tests_sv2::{
    interceptor::MessageDirection, start_pool, start_sniffer, start_template_provider,
    sv2_tp_config, template_provider::DifficultyLevel,
};
use std::collections::BTreeSet;
use std::time::Duration;
use stratum_apps::stratum_core::mining_sv2::*;
use stratum_apps::stratum_core::parsers_sv2::{AnyMessageOwned, MiningOwned};
use tokio::time::Instant;

// Standard channels without REQUIRES_STANDARD_JOBS: after the per-channel job sent at
// open, the mining server only announces new work as NewExtendedMiningJob addressed to
// the group channel, so shares referencing such a job prove the miner derives standard
// jobs from the group broadcast.
#[tokio::test]
async fn test_mining_client_standard_channels_in_group_mode() {
    let _ = tracing_subscriber::fmt().try_init();

    let (_tp, tp_addr) = start_template_provider(None, DifficultyLevel::Low);
    let (_pool, pool_addr, _) = start_pool(sv2_tp_config(tp_addr), vec![], vec![], false).await;
    let (sniffer, sniffer_addr) = start_sniffer("", pool_addr, false, vec![], None);

    // Give sniffer time to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let config = Sv2CpuMinerConfig {
        server_addr: sniffer_addr,
        auth_pk: None,
        n_extended_channels: 0,
        n_standard_channels: 2,
        requires_standard_jobs: false,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: false,
        cpu_usage_percent: 100,
        nominal_hashrate_multiplier: 1.0,
        log_file: None,
    };

    let client = Sv2CpuMiner::new(config).await;
    let mut client_clone = client.clone();
    tokio::spawn(async move {
        client_clone.start().await.unwrap();
    });

    // Wait for client to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
        )
        .await;
    let group_channel_id = loop {
        match sniffer.next_message_from_upstream() {
            Some((
                _,
                AnyMessageOwned::Mining(MiningOwned::OpenStandardMiningChannelSuccess(success)),
            )) => break success.group_channel_id,
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .await;
    let mut per_channel_job_ids = BTreeSet::new();
    let mut group_job_ids = BTreeSet::new();
    let mut submitted_job_ids = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !references_a_group_only_job(&submitted_job_ids, &group_job_ids, &per_channel_job_ids)
        && Instant::now() < deadline
    {
        // jobs first: a share always follows the announcement of the job it references
        while let Some((_, message)) = sniffer.next_message_from_upstream() {
            match message {
                AnyMessageOwned::Mining(MiningOwned::NewMiningJob(job)) => {
                    per_channel_job_ids.insert(job.job_id);
                }
                AnyMessageOwned::Mining(MiningOwned::NewExtendedMiningJob(job)) => {
                    assert_eq!(
                        job.channel_id, group_channel_id,
                        "extended jobs must be addressed to the group channel"
                    );
                    group_job_ids.insert(job.job_id);
                }
                _ => {}
            }
        }
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesStandard(share)))) => {
                submitted_job_ids.insert(share.job_id);
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert!(
        references_a_group_only_job(&submitted_job_ids, &group_job_ids, &per_channel_job_ids),
        "no share referenced a job announced only through the group channel; \
         submitted {submitted_job_ids:?}, group jobs {group_job_ids:?}, per-channel jobs {per_channel_job_ids:?}"
    );

    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;
}

fn references_a_group_only_job(
    submitted: &BTreeSet<u32>,
    group: &BTreeSet<u32>,
    per_channel: &BTreeSet<u32>,
) -> bool {
    submitted
        .iter()
        .any(|job_id| group.contains(job_id) && !per_channel.contains(job_id))
}
