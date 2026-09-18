use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use cpu_miner_sv2::error::Sv2CpuMinerError;
use integration_tests_sv2::{
    mock_roles::{MockUpstream, WithSetup},
    utils::get_available_address,
};
use std::net::SocketAddr;
use std::time::Duration;
use stratum_apps::stratum_core::binary_sv2::Sv2OptionOwned;
use stratum_apps::stratum_core::common_messages_sv2::Protocol;
use stratum_apps::stratum_core::mining_sv2::*;
use stratum_apps::stratum_core::parsers_sv2::{AnyMessageOwned, MiningOwned};

// On a connection that declared REQUIRES_STANDARD_JOBS, a mining server may send neither
// NewExtendedMiningJob nor SetGroupChannel; either ends the run with StandardJobsOnly.

fn config(server_addr: SocketAddr) -> Sv2CpuMinerConfig {
    Sv2CpuMinerConfig {
        server_addr,
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
    }
}

/// Runs the miner against a mock mining server that pushes `message` right after the
/// handshake, and returns what start() came back with.
async fn run_receiving(message: AnyMessageOwned) -> Result<(), Sv2CpuMinerError> {
    let mock_upstream_addr = get_available_address();
    let mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0),
    )
    .start()
    .await;
    mock_upstream_sender.send(message).await.unwrap();

    let mut client = Sv2CpuMiner::new(config(mock_upstream_addr)).await;
    tokio::time::timeout(Duration::from_secs(10), client.start())
        .await
        .expect("start() must return once the forbidden message arrives")
}

#[tokio::test]
async fn test_mining_client_refuses_an_extended_job() {
    let _ = tracing_subscriber::fmt().try_init();

    // the miner refuses the message before looking at the job, so any coinbase bytes do
    let extended_job = AnyMessageOwned::Mining(MiningOwned::NewExtendedMiningJob(
        NewExtendedMiningJobOwned {
            channel_id: 1,
            job_id: 10,
            min_ntime: Sv2OptionOwned::new(None),
            version: 536870912,
            version_rolling_allowed: true,
            merkle_path: vec![].try_into().unwrap(),
            coinbase_tx_prefix: vec![0_u8; 8].try_into().unwrap(),
            coinbase_tx_suffix: vec![0_u8; 8].try_into().unwrap(),
        },
    ));

    let result = run_receiving(extended_job).await;
    assert!(
        matches!(
            result,
            Err(Sv2CpuMinerError::StandardJobsOnly("NewExtendedMiningJob"))
        ),
        "start() returned {result:?}"
    );
}

#[tokio::test]
async fn test_mining_client_refuses_set_group_channel() {
    let _ = tracing_subscriber::fmt().try_init();

    let set_group_channel =
        AnyMessageOwned::Mining(MiningOwned::SetGroupChannel(SetGroupChannelOwned {
            group_channel_id: 1,
            channel_ids: vec![2].try_into().unwrap(),
        }));

    let result = run_receiving(set_group_channel).await;
    assert!(
        matches!(
            result,
            Err(Sv2CpuMinerError::StandardJobsOnly("SetGroupChannel"))
        ),
        "start() returned {result:?}"
    );
}
