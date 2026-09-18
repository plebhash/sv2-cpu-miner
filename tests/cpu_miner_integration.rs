// This file contains integration tests for the `Sv2CpuMiner` module.
//
// `Sv2CpuMiner` is a Stratum V2 mining client that hashes on the CPU.
use cpu_miner_sv2::{client::Sv2CpuMiner, config::Sv2CpuMinerConfig};
use integration_tests_sv2::{
    interceptor::MessageDirection,
    mock_roles::{MockUpstream, WithSetup},
    template_provider::DifficultyLevel,
    utils::get_available_address,
    *,
};
use std::{collections::BTreeSet, time::Duration};
use stratum_apps::stratum_core::{
    binary_sv2::Sv2OptionOwned,
    common_messages_sv2::{Protocol, *},
    mining_sv2::*,
    parsers_sv2::{AnyMessageOwned, MiningOwned},
};
use tokio::time::Instant;

// This test starts a Template Provider, a Pool and a Sniffer, and checks that a miner with one
// standard channel opens it and submits a share the Pool accepts. The Sniffer is a proxy between
// the Upstream(Pool) and the Downstream(miner).
#[tokio::test]
async fn test_mining_client_one_standard_channel() {
    start_tracing();

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
        .wait_for_message_type(MessageDirection::ToDownstream, MESSAGE_TYPE_NEW_MINING_JOB)
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
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;
}

// The same for one extended channel. REQUIRES_STANDARD_JOBS is not declared, so the Pool serves
// the channel with NewExtendedMiningJob.
#[tokio::test]
async fn test_mining_client_one_extended_channel() {
    start_tracing();

    let (_tp, tp_addr) = start_template_provider(None, DifficultyLevel::Low);
    let (_pool, pool_addr, _) = start_pool(sv2_tp_config(tp_addr), vec![], vec![], false).await;
    let (sniffer, sniffer_addr) = start_sniffer("", pool_addr, false, vec![], None);

    // Give sniffer time to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let config = Sv2CpuMinerConfig {
        server_addr: sniffer_addr,
        auth_pk: None,
        n_extended_channels: 1,
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
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
        )
        .await;
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;
}

// One standard + one extended channel on a single connection. Without REQUIRES_STANDARD_JOBS the
// mining server may serve every channel through the group channel: per-channel jobs at
// channel-open time, then group-addressed NewExtendedMiningJob and SetNewPrevHash for all members.
#[tokio::test]
async fn test_mining_client_mixed_channels() {
    start_tracing();

    let (_tp, tp_addr) = start_template_provider(None, DifficultyLevel::Low);
    let (_pool, pool_addr, _) = start_pool(sv2_tp_config(tp_addr), vec![], vec![], false).await;
    let (sniffer, sniffer_addr) = start_sniffer("", pool_addr, false, vec![], None);

    // Give sniffer time to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let config = Sv2CpuMinerConfig {
        server_addr: sniffer_addr,
        auth_pk: None,
        n_extended_channels: 1,
        n_standard_channels: 1,
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
    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;
}

// Two standard channels on one connection: both get opened, and both submit shares.
#[tokio::test]
async fn test_mining_client_two_standard_channels() {
    start_tracing();

    let (_tp, tp_addr) = start_template_provider(None, DifficultyLevel::Low);
    let (_pool, pool_addr, _) = start_pool(sv2_tp_config(tp_addr), vec![], vec![], false).await;
    let (sniffer, sniffer_addr) = start_sniffer("", pool_addr, false, vec![], None);

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
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
        )
        .await;
    let mut opened_channel_ids = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while opened_channel_ids.len() < 2 && Instant::now() < deadline {
        match sniffer.next_message_from_upstream() {
            Some((
                _,
                AnyMessageOwned::Mining(MiningOwned::OpenStandardMiningChannelSuccess(success)),
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
        "both standard channels must be opened"
    );

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .await;
    let mut submitting_channel_ids = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while submitting_channel_ids.len() < 2 && Instant::now() < deadline {
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesStandard(share)))) => {
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

// The same for two extended channels.
#[tokio::test]
async fn test_mining_client_two_extended_channels() {
    start_tracing();

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

// Standard channels without REQUIRES_STANDARD_JOBS: after the per-channel job sent at open, the
// mining server only announces new work as NewExtendedMiningJob addressed to the group channel, so
// a share referencing such a job proves the miner derives standard jobs from the group broadcast.
#[tokio::test]
async fn test_mining_client_standard_channels_in_group_mode() {
    start_tracing();

    let (_tp, tp_addr) = start_template_provider(None, DifficultyLevel::Low);
    let (_pool, pool_addr, _) = start_pool(sv2_tp_config(tp_addr), vec![], vec![], false).await;
    let (sniffer, sniffer_addr) = start_sniffer("", pool_addr, false, vec![], None);

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
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
        )
        .await;
    let group_channel_id = loop {
        match sniffer.next_message_from_upstream() {
            Some((
                _,
                AnyMessageOwned::Mining(MiningOwned::OpenStandardMiningChannelSuccess(success)),
            )) => break success.group_channel_id,
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .await;
    let mut per_channel_job_ids = BTreeSet::new();
    let mut group_job_ids = BTreeSet::new();
    let mut share_from_a_group_job = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    while share_from_a_group_job.is_none() && Instant::now() < deadline {
        // jobs first: a share always follows the announcement of the job it references
        while let Some((_, message)) = sniffer.next_message_from_upstream() {
            match message {
                AnyMessageOwned::Mining(MiningOwned::NewMiningJob(job)) => {
                    per_channel_job_ids.insert(job.job_id);
                }
                AnyMessageOwned::Mining(MiningOwned::NewExtendedMiningJob(job)) => {
                    assert_eq!(
                        job.channel_id, group_channel_id,
                        "extended jobs must be addressed to the group channel"
                    );
                    group_job_ids.insert(job.job_id);
                }
                _ => {}
            }
        }
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesStandard(share)))) => {
                if group_job_ids.contains(&share.job_id)
                    && !per_channel_job_ids.contains(&share.job_id)
                {
                    share_from_a_group_job = Some(share.job_id);
                }
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert!(
        share_from_a_group_job.is_some(),
        "no share referenced a job announced only through the group channel; \
         group jobs {group_job_ids:?}, per-channel jobs {per_channel_job_ids:?}"
    );

    sniffer
        .wait_for_message_type(
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;
}

// With single_submit the channel submits one share and then stops hashing: after the first
// SubmitSharesStandard nothing else may reach the mining server, even though the server keeps
// sending work.
#[tokio::test]
async fn test_mining_client_single_submit() {
    start_tracing();

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

    // The channel target is little-endian, so zeroing its top two bytes asks for a hash with 16
    // leading zero bits: about one share per 65536 hashes, a couple per second on a CPU. A
    // [0xff; 32] target would instead make every hash a share and flood the queue this test
    // drains.
    let mut target = [0xff_u8; 32];
    target[30] = 0;
    target[31] = 0;
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(
            MiningOwned::OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccessOwned {
                request_id: 0,
                channel_id: 2,
                target: target.into(),
                extranonce_prefix: vec![0_u8; 8].try_into().unwrap(),
                group_channel_id: 1,
            }),
        ))
        .await
        .unwrap();
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::NewMiningJob(
            NewMiningJobOwned {
                channel_id: 2,
                job_id: 10,
                min_ntime: Sv2OptionOwned::new(None),
                version: 536870912,
                merkle_root: [0_u8; 32].into(),
            },
        )))
        .await
        .unwrap();
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::SetNewPrevHash(
            SetNewPrevHashOwned {
                channel_id: 2,
                job_id: 10,
                prev_hash: [0_u8; 32].into(),
                min_ntime: 1745596970,
                nbits: 453040064,
            },
        )))
        .await
        .unwrap();

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

// Every channel advertises the measured hashrate times nominal_hashrate_multiplier, split evenly
// across the channels. A mock mining server is enough: only the open requests matter.
#[tokio::test]
async fn test_mining_client_nominal_hashrate_multiplier() {
    start_tracing();

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

// shutdown() on any clone stops a running miner, and start() then returns Ok.
#[tokio::test]
async fn test_mining_client_shutdown_returns_ok() {
    start_tracing();

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
            MessageDirection::ToDownstream,
            MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
        )
        .await;

    client.shutdown().await;

    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("start() must return once shutdown() was called")
        .expect("the miner task must not panic");
    assert!(result.is_ok(), "start() returned {result:?}");
}

// A block header timestamp claims when the header was built, so a miner may only advance it as
// real seconds pass: the Sv2 spec bounds a share's ntime at the job's min_ntime plus the seconds
// elapsed since the message that activated it. Mining one static job for a few seconds must
// therefore produce timestamps that move, but never ahead of the clock.
#[tokio::test]
async fn test_mining_client_advances_ntime_with_the_clock() {
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
        n_standard_channels: 1,
        requires_standard_jobs: true,
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

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .await;

    // about one share per 4096 hashes, a few dozen per second, so the window below spans
    // several timestamps
    let mut target = [0xff_u8; 32];
    target[30] = 0x0f;
    target[31] = 0;
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(
            MiningOwned::OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccessOwned {
                request_id: 0,
                channel_id: 2,
                target: target.into(),
                extranonce_prefix: vec![0_u8; 8].try_into().unwrap(),
                group_channel_id: 1,
            }),
        ))
        .await
        .unwrap();
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::NewMiningJob(
            NewMiningJobOwned {
                channel_id: 2,
                job_id: 10,
                min_ntime: Sv2OptionOwned::new(None),
                version: 536870912,
                merkle_root: [0_u8; 32].into(),
            },
        )))
        .await
        .unwrap();

    // the job is activated by this message, so its min_ntime is what the miner rolls from and
    // this instant bounds how far the miner's own clock can have advanced
    let job_min_ntime = 1745596970_u32;
    let activated_at = Instant::now();
    mock_upstream_sender
        .send(AnyMessageOwned::Mining(MiningOwned::SetNewPrevHash(
            SetNewPrevHashOwned {
                channel_id: 2,
                job_id: 10,
                prev_hash: [0_u8; 32].into(),
                min_ntime: job_min_ntime,
                nbits: 453040064,
            },
        )))
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut ntimes = Vec::new();
    while Instant::now() < deadline {
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesStandard(share)))) => {
                ntimes.push(share.ntime);
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let elapsed_seconds = activated_at.elapsed().as_secs() as u32;

    assert!(!ntimes.is_empty(), "no share was submitted");
    assert!(
        ntimes.iter().all(|&ntime| ntime >= job_min_ntime),
        "a share predates the job it references: {ntimes:?}"
    );
    assert!(
        ntimes
            .iter()
            .all(|&ntime| ntime <= job_min_ntime + elapsed_seconds),
        "ntime ran ahead of the clock after {elapsed_seconds}s: {ntimes:?}"
    );
    assert!(
        ntimes.iter().any(|&ntime| ntime > job_min_ntime),
        "ntime never advanced although seconds passed: {ntimes:?}"
    );
}
