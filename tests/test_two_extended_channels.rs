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

// Two extended channels on one connection: both get opened, and both submit shares the
// mining server accepts.
#[tokio::test]
async fn test_mining_client_two_extended_channels() {
    let _ = tracing_subscriber::fmt().try_init();

    let (_tp, tp_addr) = start_template_provider(None, DifficultyLevel::Low);
    let (_pool, pool_addr, _) = start_pool(sv2_tp_config(tp_addr), vec![], vec![], false).await;
    let (sniffer, sniffer_addr) = start_sniffer("", pool_addr, false, vec![], None);

    // Give sniffer time to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let config = Sv2CpuMinerConfig {
        server_addr: sniffer_addr,
        auth_pk: None,
        n_extended_channels: 2,
        n_standard_channels: 0,
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
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
        )
        .await;
    let mut opened_channel_ids = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while opened_channel_ids.len() < 2 && Instant::now() < deadline {
        match sniffer.next_message_from_upstream() {
            Some((
                _,
                AnyMessageOwned::Mining(MiningOwned::OpenExtendedMiningChannelSuccess(success)),
            )) => {
                opened_channel_ids.insert(success.channel_id);
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert_eq!(
        opened_channel_ids.len(),
        2,
        "both extended channels must be opened"
    );

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
        )
        .await;
    let mut submitting_channel_ids = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while submitting_channel_ids.len() < 2 && Instant::now() < deadline {
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesExtended(share)))) => {
                submitting_channel_ids.insert(share.channel_id);
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert_eq!(
        submitting_channel_ids, opened_channel_ids,
        "every opened channel must submit shares"
    );

    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;
}
