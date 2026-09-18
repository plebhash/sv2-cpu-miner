// This file contains integration tests for how `Sv2CpuMiner` handles mining messages that the SRI
// Pool does not produce: several groups on one connection, SetGroupChannel, and messages a
// connection that declared REQUIRES_STANDARD_JOBS may not receive. A mock mining server plays them
// out, with a Sniffer in front when the miner's own messages have to be observed.
use cpu_miner_sv2::{client::Sv2CpuMiner, config::Sv2CpuMinerConfig, error::Sv2CpuMinerError};
use integration_tests_sv2::{
    interceptor::MessageDirection,
    mock_roles::{MockUpstream, WithSetup},
    utils::get_available_address,
    *,
};
use std::{collections::BTreeSet, time::Duration};
use stratum_apps::stratum_core::{
    binary_sv2::Sv2OptionOwned,
    common_messages_sv2::Protocol,
    mining_sv2::*,
    parsers_sv2::{AnyMessageOwned, MiningOwned},
};
use tokio::time::Instant;

// A coinbase that parses once a 32-byte extranonce is inserted, so a standard channel can derive
// its own merkle root from a group job. The bytes come from the channels_sv2 client tests.
const COINBASE_TX_PREFIX: &[u8] = &[
    2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 255, 255, 255, 255, 34, 82, 0,
];
const COINBASE_TX_SUFFIX: &[u8] = &[
    255, 255, 255, 255, 2, 0, 242, 5, 42, 1, 0, 0, 0, 22, 0, 20, 235, 225, 183, 220, 194, 147, 204,
    170, 14, 231, 67, 168, 111, 137, 223, 130, 88, 194, 8, 252, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106,
    36, 170, 33, 169, 237, 226, 246, 28, 63, 113, 209, 222, 253, 63, 169, 153, 223, 163, 105, 83,
    117, 92, 105, 6, 137, 121, 153, 98, 180, 139, 235, 216, 54, 151, 78, 140, 249, 0, 0, 0, 0,
];

// Every channel belongs to a group and a mining server may run several on one connection
// (sv2-spec 5.2.3), so a job addressed to a group must reach that group's members and no others.
// Channel 2 is in group 1 and channel 3 in group 4; only channel 2 may mine group 1's job.
#[tokio::test]
async fn test_mining_client_group_job_reaches_only_its_members() {
    start_tracing();

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
        n_standard_channels: 2,
        requires_standard_jobs: false,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: false,
        cpu_usage_percent: 100,
        nominal_hashrate_multiplier: 1.0,
        log_file: None,
    };

    let mut client = Sv2CpuMiner::new(config).await;
    let mut client_clone = client.clone();
    let run = tokio::spawn(async move { client_clone.start().await });

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .await;

    // The channel target is little-endian, so zeroing its top two bytes asks for a hash with 16
    // leading zero bits: about one share per 65536 hashes, a couple per second on a CPU. A
    // [0xff; 32] target would instead make every hash a share and flood the queue this test
    // drains.
    let mut target = [0xff_u8; 32];
    target[30] = 0;
    target[31] = 0;
    for (request_id, channel_id, group_channel_id) in [(0, 2, 1), (1, 3, 4)] {
        mock_upstream_sender
            .send(AnyMessageOwned::Mining(
                MiningOwned::OpenStandardMiningChannelSuccess(
                    OpenStandardMiningChannelSuccessOwned {
                        request_id,
                        channel_id,
                        target: target.into(),
                        // 32 bytes: the group job's coinbase carries a 32-byte extranonce
                        extranonce_prefix: vec![0_u8; 32].try_into().unwrap(),
                        group_channel_id,
                    },
                ),
            ))
            .await
            .unwrap();
    }
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::NewExtendedMiningJob(
            NewExtendedMiningJobOwned {
                channel_id: 1,
                job_id: 10,
                min_ntime: Sv2OptionOwned::new(None),
                version: 536870912,
                version_rolling_allowed: true,
                merkle_path: vec![].try_into().unwrap(),
                coinbase_tx_prefix: COINBASE_TX_PREFIX.to_vec().try_into().unwrap(),
                coinbase_tx_suffix: COINBASE_TX_SUFFIX.to_vec().try_into().unwrap(),
            },
        )))
        .await
        .unwrap();
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::SetNewPrevHash(
            SetNewPrevHashOwned {
                channel_id: 1,
                job_id: 10,
                prev_hash: [0_u8; 32].into(),
                min_ntime: 1745596970,
                nbits: 453040064,
            },
        )))
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut submitting_channel_ids = Vec::new();
    while Instant::now() < deadline {
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesStandard(share)))) => {
                submitting_channel_ids.push(share.channel_id);
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert!(
        !submitting_channel_ids.is_empty(),
        "channel 2 never submitted a share"
    );
    assert!(
        submitting_channel_ids
            .iter()
            .all(|&channel_id| channel_id == 2),
        "a channel outside group 1 submitted: {submitting_channel_ids:?}"
    );

    client.shutdown().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}

// SetGroupChannel redefines a group as exactly the channels it lists (sv2-spec 5.3.22). After
// group 4 becomes {2, 3}, work addressed to group 4 must reach both channels.
#[tokio::test]
async fn test_mining_client_set_group_channel_regroups() {
    start_tracing();

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
        n_standard_channels: 2,
        requires_standard_jobs: false,
        user_identity: "test".to_string(),
        device_id: "test".to_string(),
        single_submit: false,
        cpu_usage_percent: 100,
        nominal_hashrate_multiplier: 1.0,
        log_file: None,
    };

    let mut client = Sv2CpuMiner::new(config).await;
    let mut client_clone = client.clone();
    let run = tokio::spawn(async move { client_clone.start().await });

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .await;

    // The channel target is little-endian, so zeroing its top two bytes asks for a hash with 16
    // leading zero bits: about one share per 65536 hashes, a couple per second on a CPU. A
    // [0xff; 32] target would instead make every hash a share and flood the queue this test
    // drains.
    let mut target = [0xff_u8; 32];
    target[30] = 0;
    target[31] = 0;
    for (request_id, channel_id, group_channel_id) in [(0, 2, 1), (1, 3, 4)] {
        mock_upstream_sender
            .send(AnyMessageOwned::Mining(
                MiningOwned::OpenStandardMiningChannelSuccess(
                    OpenStandardMiningChannelSuccessOwned {
                        request_id,
                        channel_id,
                        target: target.into(),
                        extranonce_prefix: vec![0_u8; 32].try_into().unwrap(),
                        group_channel_id,
                    },
                ),
            ))
            .await
            .unwrap();
    }

    // group 4 becomes {2, 3}, and its job 20 must then reach both channels
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::SetGroupChannel(
            SetGroupChannelOwned {
                group_channel_id: 4,
                channel_ids: vec![2, 3].try_into().unwrap(),
            },
        )))
        .await
        .unwrap();
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::NewExtendedMiningJob(
            NewExtendedMiningJobOwned {
                channel_id: 4,
                job_id: 20,
                min_ntime: Sv2OptionOwned::new(None),
                version: 536870912,
                version_rolling_allowed: true,
                merkle_path: vec![].try_into().unwrap(),
                coinbase_tx_prefix: COINBASE_TX_PREFIX.to_vec().try_into().unwrap(),
                coinbase_tx_suffix: COINBASE_TX_SUFFIX.to_vec().try_into().unwrap(),
            },
        )))
        .await
        .unwrap();
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::SetNewPrevHash(
            SetNewPrevHashOwned {
                channel_id: 4,
                job_id: 20,
                prev_hash: [0_u8; 32].into(),
                min_ntime: 1745596970,
                nbits: 453040064,
            },
        )))
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut submitting_channel_ids = BTreeSet::new();
    while submitting_channel_ids.len() < 2 && Instant::now() < deadline {
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesStandard(share)))) => {
                if share.job_id == 20 {
                    submitting_channel_ids.insert(share.channel_id);
                }
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert_eq!(
        submitting_channel_ids,
        BTreeSet::from([2, 3]),
        "both channels must mine the regrouped job"
    );

    client.shutdown().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}

// A connection that declared REQUIRES_STANDARD_JOBS may not receive NewExtendedMiningJob: the
// client said it cannot process extended jobs, so the run ends naming the offending message.
#[tokio::test]
async fn test_mining_client_refuses_an_extended_job_when_standard_jobs_are_required() {
    start_tracing();

    let mock_upstream_addr = get_available_address();
    let mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0),
    )
    .start()
    .await;
    // the miner refuses the message before looking at the job, so any coinbase bytes do
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::NewExtendedMiningJob(
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
        )))
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
        .expect("start() must return once the forbidden message arrives");
    assert!(
        matches!(
            result,
            Err(Sv2CpuMinerError::StandardJobsOnly("NewExtendedMiningJob"))
        ),
        "start() returned {result:?}"
    );
}

// SetGroupChannel is forbidden on the same connections, since regrouping only serves group jobs.
#[tokio::test]
async fn test_mining_client_refuses_set_group_channel_when_standard_jobs_are_required() {
    start_tracing();

    let mock_upstream_addr = get_available_address();
    let mock_upstream_sender = MockUpstream::new(
        mock_upstream_addr,
        WithSetup::yes_with_defaults(Protocol::MiningProtocol, 0),
    )
    .start()
    .await;
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::SetGroupChannel(
            SetGroupChannelOwned {
                group_channel_id: 1,
                channel_ids: vec![2].try_into().unwrap(),
            },
        )))
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
        .expect("start() must return once the forbidden message arrives");
    assert!(
        matches!(
            result,
            Err(Sv2CpuMinerError::StandardJobsOnly("SetGroupChannel"))
        ),
        "start() returned {result:?}"
    );
}

// A refused channel is logged, not fatal: the run stays up until shutdown() ends it.
#[tokio::test]
async fn test_mining_client_open_channel_error_keeps_running() {
    start_tracing();

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
