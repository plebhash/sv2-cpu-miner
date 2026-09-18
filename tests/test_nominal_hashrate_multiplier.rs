use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use integration_tests_sv2::{
    interceptor::MessageDirection,
    mock_roles::{MockUpstream, WithSetup},
    start_sniffer,
    utils::get_available_address,
};
use std::time::Duration;
use stratum_apps::stratum_core::common_messages_sv2::Protocol;
use stratum_apps::stratum_core::mining_sv2::*;
use stratum_apps::stratum_core::parsers_sv2::{AnyMessageOwned, MiningOwned};
use tokio::time::Instant;

// Every channel advertises the measured hashrate times nominal_hashrate_multiplier, split
// evenly across the channels. A mock mining server is enough: only the open requests matter.
#[tokio::test]
async fn test_mining_client_nominal_hashrate_multiplier() {
    let _ = tracing_subscriber::fmt().try_init();

    let mock_upstream_addr = get_available_address();
    let _mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0),
    )
    .start()
    .await;
    let (sniffer, sniffer_addr) = start_sniffer("", mock_upstream_addr, false, vec![], Some(10));

    // Give sniffer time to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let config = Sv2CpuMinerConfig {
        server_addr: sniffer_addr,
        auth_pk: None,
        n_extended_channels: 0,
        n_standard_channels: 2,
        requires_standard_jobs: true,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: false,
        cpu_usage_percent: 100,
        nominal_hashrate_multiplier: 2.5,
        log_file: None,
    };

    let client = Sv2CpuMiner::new(config).await;
    let expected_per_channel = client.nominal_hashrate() * 2.5 / 2.0;
    let mut client_clone = client.clone();
    tokio::spawn(async move {
        client_clone.start().await.unwrap();
    });

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .await;
    let mut advertised = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while advertised.len() < 2 && Instant::now() < deadline {
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::OpenStandardMiningChannel(open)))) => {
                advertised.push(open.nominal_hash_rate);
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }

    assert_eq!(advertised.len(), 2, "both channels must be requested");
    for nominal_hash_rate in advertised {
        let relative_error =
            ((nominal_hash_rate - expected_per_channel) / expected_per_channel).abs();
        assert!(
            relative_error < 1e-4,
            "advertised {nominal_hash_rate} H/s, expected {expected_per_channel} H/s"
        );
    }
}
