use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use integration_tests_sv2::{
    interceptor::MessageDirection, start_pool, start_sniffer, start_template_provider,
    sv2_tp_config, template_provider::DifficultyLevel,
};
use std::time::Duration;
use stratum_apps::stratum_core::mining_sv2::*;

// With single_submit the channel submits one share and then stops hashing: after the
// first SubmitSharesStandard nothing else may reach the mining server, even though the
// server keeps sending work.
#[tokio::test]
async fn test_mining_client_single_submit() {
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
        n_standard_channels: 1,
        requires_standard_jobs: true,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: true,
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

    assert!(
        sniffer
            .wait_for_message_type_and_clean_queue(
                MessageDirection::ToUpstream,
                MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
            )
            .await
    );
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;

    assert!(
        sniffer
            .assert_message_not_present(
                MessageDirection::ToUpstream,
                MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
                Duration::from_secs(3),
            )
            .await,
        "a second share was submitted although single_submit is set"
    );
}

// The same guarantee against a mining server that never replaces the job: a miner that kept
// hashing after its first share would keep submitting, since the target stays reachable.
#[tokio::test]
async fn test_mining_client_single_submit_with_a_static_job() {
    use integration_tests_sv2::mock_roles::{MockUpstream, WithSetup};
    use integration_tests_sv2::utils::get_available_address;
    use stratum_apps::stratum_core::binary_sv2::Sv2OptionOwned;
    use stratum_apps::stratum_core::common_messages_sv2::Protocol;
    use stratum_apps::stratum_core::parsers_sv2::{AnyMessageOwned, MiningOwned};

    let _ = tracing_subscriber::fmt().try_init();

    let mock_upstream_addr = get_available_address();
    let mock_upstream_sender = MockUpstream::new(
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
        n_standard_channels: 1,
        requires_standard_jobs: true,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: true,
        cpu_usage_percent: 100,
        nominal_hashrate_multiplier: 1.0,
        log_file: None,
    };

    let client = Sv2CpuMiner::new(config).await;
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

    // a target reachable a few times per second, so the assertion window stays cheap
    let mut target = [0xff_u8; 32];
    target[30] = 0;
    target[31] = 0;
    let open_channel_success = AnyMessageOwned::Mining(
        MiningOwned::OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccessOwned {
            request_id: 0,
            channel_id: 2,
            target: target.into(),
            extranonce_prefix: vec![0_u8; 8].try_into().unwrap(),
            group_channel_id: 1,
        }),
    );
    let future_job = AnyMessageOwned::Mining(MiningOwned::NewMiningJob(NewMiningJobOwned {
        channel_id: 2,
        job_id: 10,
        min_ntime: Sv2OptionOwned::new(None),
        version: 536870912,
        merkle_root: [0_u8; 32].into(),
    }));
    let set_new_prev_hash =
        AnyMessageOwned::Mining(MiningOwned::SetNewPrevHash(SetNewPrevHashOwned {
            channel_id: 2,
            job_id: 10,
            prev_hash: [0_u8; 32].into(),
            min_ntime: 1745596970,
            nbits: 453040064,
        }));
    for message in [open_channel_success, future_job, set_new_prev_hash] {
        mock_upstream_sender.send(message).await.unwrap();
    }

    assert!(
        sniffer
            .wait_for_message_type_and_clean_queue(
                MessageDirection::ToUpstream,
                MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
            )
            .await
    );
    assert!(
        sniffer
            .assert_message_not_present(
                MessageDirection::ToUpstream,
                MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
                Duration::from_secs(3),
            )
            .await,
        "a second share was submitted although single_submit is set"
    );
}
