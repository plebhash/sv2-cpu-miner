use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use cpu_miner_sv2::error::Sv2CpuMinerError;
use integration_tests_sv2::{
    mock_roles::{MockUpstream, WithSetup},
    utils::get_available_address,
};
use std::time::Duration;
use stratum_apps::stratum_core::common_messages_sv2::Protocol;

// A mining server that answers SetupConnection with SetupConnection.Error ends the run
// with SetupConnectionFailed. The mock answers with an error whenever the protocol it
// expects differs from the one offered.
#[tokio::test]
async fn test_mining_client_setup_connection_error() {
    let _ = tracing_subscriber::fmt().try_init();

    let mock_upstream_addr = get_available_address();
    let _mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::TemplateDistributionProtocol, 0),
    )
    .start()
    .await;

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
    let result = tokio::time::timeout(Duration::from_secs(10), client.start())
        .await
        .expect("start() must return once the handshake is refused");
    assert!(
        matches!(result, Err(Sv2CpuMinerError::SetupConnectionFailed)),
        "start() returned {result:?}"
    );
}
