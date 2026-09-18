use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use cpu_miner_sv2::error::Sv2CpuMinerError;
use integration_tests_sv2::{
    interceptor::{MessageDirection, ReplaceMessage},
    mock_roles::{MockUpstream, WithSetup},
    start_sniffer,
    utils::get_available_address,
};
use std::net::SocketAddr;
use std::time::Duration;
use stratum_apps::stratum_core::common_messages_sv2::*;
use stratum_apps::stratum_core::mining_sv2::*;
use stratum_apps::stratum_core::parsers_sv2::{AnyMessageOwned, CommonMessagesOwned, MiningOwned};

// The miner must verify the mining server's SetupConnection.Success and refuse parameters
// it cannot honour. Each case ends start() with the matching error.

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

async fn run_against(server_addr: SocketAddr) -> Result<(), Sv2CpuMinerError> {
    let mut client = Sv2CpuMiner::new(config(server_addr)).await;
    tokio::time::timeout(Duration::from_secs(10), client.start())
        .await
        .expect("start() must return once the handshake is refused")
}

#[tokio::test]
async fn test_mining_client_rejects_undefined_flags() {
    let _ = tracing_subscriber::fmt().try_init();

    let mock_upstream_addr = get_available_address();
    let _mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0b100),
    )
    .start()
    .await;

    let result = run_against(mock_upstream_addr).await;
    assert!(
        matches!(result, Err(Sv2CpuMinerError::SetupConnectionMismatch(_))),
        "start() returned {result:?}"
    );
}

#[tokio::test]
async fn test_mining_client_rejects_required_extended_channels_with_standard_channels_configured() {
    let _ = tracing_subscriber::fmt().try_init();

    let mock_upstream_addr = get_available_address();
    let _mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0b10),
    )
    .start()
    .await;

    let result = run_against(mock_upstream_addr).await;
    assert!(
        matches!(result, Err(Sv2CpuMinerError::SetupConnectionMismatch(_))),
        "start() returned {result:?}"
    );
}

#[tokio::test]
async fn test_mining_client_rejects_a_first_reply_that_is_not_a_setup_connection_reply() {
    let _ = tracing_subscriber::fmt().try_init();

    // the mock never answers the handshake; the pushed message becomes the first reply
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

    let result = run_against(mock_upstream_addr).await;
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

#[tokio::test]
async fn test_mining_client_rejects_a_version_that_was_not_offered() {
    let _ = tracing_subscriber::fmt().try_init();

    // the mock always answers with version 2, so a sniffer rewrites its reply
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

    let result = run_against(sniffer_addr).await;
    assert!(
        matches!(result, Err(Sv2CpuMinerError::SetupConnectionMismatch(_))),
        "start() returned {result:?}"
    );
}
