use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use integration_tests_sv2::{
    mock_roles::{MockUpstream, WithSetup},
    utils::get_available_address,
};
use std::time::Duration;
use stratum_apps::stratum_core::common_messages_sv2::Protocol;
use stratum_apps::stratum_core::mining_sv2::*;
use stratum_apps::stratum_core::parsers_sv2::{AnyMessageOwned, MiningOwned};

// A refused channel is logged, not fatal: the run stays up until shutdown() ends it.
#[tokio::test]
async fn test_mining_client_open_channel_error_keeps_running() {
    let _ = tracing_subscriber::fmt().try_init();

    let mock_upstream_addr = get_available_address();
    let mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0),
    )
    .start()
    .await;
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(
            MiningOwned::OpenMiningChannelError(OpenMiningChannelErrorOwned {
                request_id: 0,
                error_code: "unknown-user".to_string().try_into().unwrap(),
            }),
        ))
        .await
        .unwrap();

    let config = Sv2CpuMinerConfig {
        server_addr: mock_upstream_addr,
        auth_pk: None,
        n_extended_channels: 0,
        n_standard_channels: 1,
        requires_standard_jobs: true,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: false,
        cpu_usage_percent: 100,
        nominal_hashrate_multiplier: 1.0,
        log_file: None,
    };

    let mut client = Sv2CpuMiner::new(config).await;
    let mut client_clone = client.clone();
    let mut run = tokio::spawn(async move { client_clone.start().await });

    assert!(
        tokio::time::timeout(Duration::from_secs(2), &mut run)
            .await
            .is_err(),
        "start() returned although a refused channel is not fatal"
    );

    client.shutdown().await;
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("start() must return once shutdown() was called")
        .expect("the miner task must not panic");
    assert!(result.is_ok(), "start() returned {result:?}");
}
