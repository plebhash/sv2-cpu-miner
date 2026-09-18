use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use cpu_miner_sv2::error::Sv2CpuMinerError;
use integration_tests_sv2::{
    interceptor::MessageDirection,
    mock_roles::{MockUpstream, WithSetup},
    sniffer::Sniffer,
    start_sniffer,
    utils::get_available_address,
};
use std::collections::BTreeSet;
use std::time::Duration;
use stratum_apps::stratum_core::binary_sv2::Sv2OptionOwned;
use stratum_apps::stratum_core::common_messages_sv2::Protocol;
use stratum_apps::stratum_core::mining_sv2::*;
use stratum_apps::stratum_core::parsers_sv2::{AnyMessageOwned, MiningOwned};
use tokio::task::JoinHandle;
use tokio::time::Instant;

// Every channel belongs to a group, a mining server may run several groups on one
// connection, and SetGroupChannel redefines them (sv2-spec 5.2.3 and 5.3.22). The test pool
// only ever creates one group, so a mock mining server plays out two: channel 2 in group 1
// and channel 3 in group 4, observed through a sniffer.

fn open_standard_success(
    request_id: u32,
    channel_id: u32,
    group_channel_id: u32,
) -> AnyMessageOwned {
    // a target reachable a few times per second, so the assertion windows stay cheap
    let mut target = [0xff_u8; 32];
    target[30] = 0;
    target[31] = 0;
    AnyMessageOwned::Mining(MiningOwned::OpenStandardMiningChannelSuccess(
        OpenStandardMiningChannelSuccessOwned {
            request_id,
            channel_id,
            target: target.into(),
            // 32 bytes: the group job's coinbase below carries a 32-byte extranonce
            extranonce_prefix: vec![0_u8; 32].try_into().unwrap(),
            group_channel_id,
        },
    ))
}

/// A future job whose coinbase parses once a 32-byte extranonce is inserted; the bytes come
/// from the channels_sv2 client tests.
fn group_job(channel_id: u32, job_id: u32) -> AnyMessageOwned {
    AnyMessageOwned::Mining(MiningOwned::NewExtendedMiningJob(
        NewExtendedMiningJobOwned {
            channel_id,
            job_id,
            min_ntime: Sv2OptionOwned::new(None),
            version: 536870912,
            version_rolling_allowed: true,
            merkle_path: vec![].try_into().unwrap(),
            coinbase_tx_prefix: vec![
                2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 34, 82, 0,
            ]
            .try_into()
            .unwrap(),
            coinbase_tx_suffix: vec![
                255, 255, 255, 255, 2, 0, 242, 5, 42, 1, 0, 0, 0, 22, 0, 20, 235, 225, 183, 220,
                194, 147, 204, 170, 14, 231, 67, 168, 111, 137, 223, 130, 88, 194, 8, 252, 0, 0, 0,
                0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 226, 246, 28, 63, 113, 209, 222,
                253, 63, 169, 153, 223, 163, 105, 83, 117, 92, 105, 6, 137, 121, 153, 98, 180, 139,
                235, 216, 54, 151, 78, 140, 249, 0, 0, 0, 0,
            ]
            .try_into()
            .unwrap(),
        },
    ))
}

fn set_new_prev_hash(channel_id: u32, job_id: u32) -> AnyMessageOwned {
    AnyMessageOwned::Mining(MiningOwned::SetNewPrevHash(SetNewPrevHashOwned {
        channel_id,
        job_id,
        prev_hash: [0_u8; 32].into(),
        min_ntime: 1745596970,
        nbits: 453040064,
    }))
}

/// A running miner with two standard channels, channel 2 in group 1 and channel 3 in group 4,
/// and no work yet.
async fn miner_with_two_groups() -> (
    Sniffer<'static>,
    async_channel::Sender<AnyMessageOwned>,
    Sv2CpuMiner,
    JoinHandle<Result<(), Sv2CpuMinerError>>,
) {
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

    let client = Sv2CpuMiner::new(config).await;
    let mut client_clone = client.clone();
    let run = tokio::spawn(async move { client_clone.start().await });

    sniffer
        .wait_for_message_type(
            MessageDirection::ToUpstream,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .await;
    for message in [
        open_standard_success(0, 2, 1),
        open_standard_success(1, 3, 4),
    ] {
        mock_upstream_sender.send(message).await.unwrap();
    }

    (sniffer, mock_upstream_sender, client, run)
}

/// Pops the miner's messages for up to `window`, stopping early once `done` holds for the
/// (channel_id, job_id) pairs of the shares seen so far.
async fn shares_within(
    sniffer: &Sniffer<'_>,
    window: Duration,
    done: impl Fn(&[(u32, u32)]) -> bool,
) -> Vec<(u32, u32)> {
    let deadline = Instant::now() + window;
    let mut shares = Vec::new();
    while !done(&shares) && Instant::now() < deadline {
        match sniffer.next_message_from_downstream() {
            Some((_, AnyMessageOwned::Mining(MiningOwned::SubmitSharesStandard(share)))) => {
                shares.push((share.channel_id, share.job_id));
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    shares
}

/// Stops the miner and waits for its run to finish.
async fn stop(mut client: Sv2CpuMiner, run: JoinHandle<Result<(), Sv2CpuMinerError>>) {
    client.shutdown().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}

#[tokio::test]
async fn test_mining_client_group_job_reaches_only_its_members() {
    let _ = tracing_subscriber::fmt().try_init();
    let (sniffer, mock_upstream_sender, client, run) = miner_with_two_groups().await;

    for message in [group_job(1, 10), set_new_prev_hash(1, 10)] {
        mock_upstream_sender.send(message).await.unwrap();
    }

    let shares = shares_within(&sniffer, Duration::from_secs(3), |_| false).await;
    assert!(!shares.is_empty(), "channel 2 never submitted a share");
    assert!(
        shares.iter().all(|&(channel_id, _)| channel_id == 2),
        "a channel outside group 1 submitted: {shares:?}"
    );

    stop(client, run).await;
}

#[tokio::test]
async fn test_mining_client_set_group_channel_regroups() {
    let _ = tracing_subscriber::fmt().try_init();
    let (sniffer, mock_upstream_sender, client, run) = miner_with_two_groups().await;

    for message in [group_job(1, 10), set_new_prev_hash(1, 10)] {
        mock_upstream_sender.send(message).await.unwrap();
    }
    let first_shares = shares_within(&sniffer, Duration::from_secs(5), |shares| {
        !shares.is_empty()
    })
    .await;
    assert!(
        !first_shares.is_empty(),
        "channel 2 never submitted a share"
    );

    // group 4 becomes {2, 3}, and work for group 4 must reach both channels
    let regroup = AnyMessageOwned::Mining(MiningOwned::SetGroupChannel(SetGroupChannelOwned {
        group_channel_id: 4,
        channel_ids: vec![2, 3].try_into().unwrap(),
    }));
    for message in [regroup, group_job(4, 20), set_new_prev_hash(4, 20)] {
        mock_upstream_sender.send(message).await.unwrap();
    }
    let channels_on_job_20 = |shares: &[(u32, u32)]| -> BTreeSet<u32> {
        shares
            .iter()
            .filter(|&&(_, job_id)| job_id == 20)
            .map(|&(channel_id, _)| channel_id)
            .collect()
    };
    let shares = shares_within(&sniffer, Duration::from_secs(10), |shares| {
        channels_on_job_20(shares).len() == 2
    })
    .await;
    assert_eq!(
        channels_on_job_20(&shares),
        BTreeSet::from([2, 3]),
        "shares after the regroup: {shares:?}"
    );

    stop(client, run).await;
}
