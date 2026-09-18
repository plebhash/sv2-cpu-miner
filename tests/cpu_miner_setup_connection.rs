// This file contains integration tests for how `Sv2CpuMiner` validates the mining server's reply
// to its `SetupConnection`. The spec requires the client to verify the version and feature flags
// the server chose and act accordingly, so a reply it cannot honour must end the run.
use cpu_miner_sv2::{client::Sv2CpuMiner, config::Sv2CpuMinerConfig, error::Sv2CpuMinerError};
use integration_tests_sv2::{
    interceptor::{MessageDirection, ReplaceMessage},
    mock_roles::{MockUpstream, WithSetup},
    utils::get_available_address,
    *,
};
use std::time::Duration;
use stratum_apps::stratum_core::{
    common_messages_sv2::{Protocol, *},
    mining_sv2::*,
    parsers_sv2::{AnyMessageOwned, CommonMessagesOwned, MiningOwned},
};

// A mining server that answers SetupConnection with SetupConnection.Error ends the run with
// SetupConnectionFailed. The mock answers with an error whenever the protocol it expects differs
// from the one offered.
#[tokio::test]
async fn test_mining_client_setup_connection_error() {
    start_tracing();

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

// Bit 2 is not a Mining Protocol server flag, so a Success carrying it cannot be honoured.
#[tokio::test]
async fn test_mining_client_rejects_undefined_flags() {
    start_tracing();

    let mock_upstream_addr = get_available_address();
    let _mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0b100),
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
        matches!(result, Err(Sv2CpuMinerError::SetupConnectionMismatch(_))),
        "start() returned {result:?}"
    );
}

// REQUIRES_EXTENDED_CHANNELS means the mining server refuses standard channels, so a config that
// asks for one cannot be served on this connection.
#[tokio::test]
async fn test_mining_client_rejects_required_extended_channels_with_standard_channels_configured() {
    start_tracing();

    let mock_upstream_addr = get_available_address();
    let _mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0b10),
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
        matches!(result, Err(Sv2CpuMinerError::SetupConnectionMismatch(_))),
        "start() returned {result:?}"
    );
}

// The first message on the connection must be a SetupConnection reply. Here the mock never answers
// the handshake, so the pushed OpenStandardMiningChannel.Success arrives in its place.
#[tokio::test]
async fn test_mining_client_rejects_a_first_reply_that_is_not_a_setup_connection_reply() {
    start_tracing();

    let mock_upstream_addr = get_available_address();
    let mock_upstream_sender = MockUpstream::new(mock_upstream_addr, WithSetup::no())
        .start()
        .await;
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(
            MiningOwned::OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccessOwned {
                request_id: 0,
                channel_id: 2,
                target: [0xff_u8; 32].into(),
                extranonce_prefix: vec![0_u8; 8].try_into().unwrap(),
                group_channel_id: 1,
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
    let result = tokio::time::timeout(Duration::from_secs(10), client.start())
        .await
        .expect("start() must return once the handshake is refused");
    assert!(
        matches!(
            result,
            Err(Sv2CpuMinerError::UnexpectedMessage(
                0,
                MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS
            ))
        ),
        "start() returned {result:?}"
    );
}

// The used_version must be the one the miner offered. The mock always answers with version 2, so a
// Sniffer rewrites its reply on the way down.
#[tokio::test]
async fn test_mining_client_rejects_a_version_that_was_not_offered() {
    start_tracing();

    let mock_upstream_addr = get_available_address();
    let _mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0),
    )
    .start()
    .await;
    let wrong_version = ReplaceMessage::new(
        MessageDirection::ToDownstream,
        MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
        AnyMessageOwned::Common(CommonMessagesOwned::SetupConnectionSuccess(
            SetupConnectionSuccessOwned {
                used_version: 3,
                flags: 0,
            },
        )),
    );
    let (_sniffer, sniffer_addr) = start_sniffer(
        "",
        mock_upstream_addr,
        false,
        vec![wrong_version.into()],
        Some(10),
    );

    // Give sniffer time to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let config = Sv2CpuMinerConfig {
        server_addr: sniffer_addr,
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
        matches!(result, Err(Sv2CpuMinerError::SetupConnectionMismatch(_))),
        "start() returned {result:?}"
    );
}
