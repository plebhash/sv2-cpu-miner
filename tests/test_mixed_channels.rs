use integration_tests_sv2::{
    interceptor::MessageDirection, start_pool, start_sniffer, start_template_provider,
    sv2_tp_config, template_provider::DifficultyLevel,
};
use stratum_apps::stratum_core::common_messages_sv2::*;
use stratum_apps::stratum_core::mining_sv2::*;
use sv2_cpu_miner::client::Sv2CpuMiner;
use sv2_cpu_miner::config::Sv2CpuMinerConfig;

// One standard + one extended channel on a single connection. Without
// REQUIRES_STANDARD_JOBS the pool runs the connection in group mode: per-channel
// jobs at channel-open time, then group-addressed NewExtendedMiningJob and
// SetNewPrevHash for all channels.
#[tokio::test]
async fn test_mining_client_mixed_channels() {
    let _ = tracing_subscriber::fmt().try_init();

    let (_tp, tp_addr) = start_template_provider(None, DifficultyLevel::Low);
    let (_pool, pool_addr, _) = start_pool(sv2_tp_config(tp_addr), vec![], vec![], false).await;
    let (sniffer, sniffer_addr) = start_sniffer("", pool_addr, false, vec![], None);

    // Give sniffer time to initialize
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let config = Sv2CpuMinerConfig {
        server_addr: sniffer_addr,
        auth_pk: None,
        n_extended_channels: 1,
        n_standard_channels: 1,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: false,
        cpu_usage_percent: 100,
        nominal_hashrate_multiplier: 1.0,
    };

    let client = Sv2CpuMiner::new(config).await.unwrap();
    let mut client_clone = client.clone();
    tokio::spawn(async move {
        client_clone.start().await.unwrap();
    });

    // Wait for client to be ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    sniffer
        .wait_for_message_type(MessageDirection::ToUpstream, MESSAGE_TYPE_SETUP_CONNECTION)
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
        )
        .await;
    sniffer
        .wait_for_message_type(MessageDirection::ToDownstream, MESSAGE_TYPE_NEW_MINING_JOB)
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
        )
        .await;
}
