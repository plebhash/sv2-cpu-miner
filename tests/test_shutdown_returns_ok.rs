use cpu_miner_sv2::client::Sv2CpuMiner;
use cpu_miner_sv2::config::Sv2CpuMinerConfig;
use integration_tests_sv2::{
    interceptor::MessageDirection, start_pool, start_sniffer, start_template_provider,
    sv2_tp_config, template_provider::DifficultyLevel,
};
use std::time::Duration;
use stratum_apps::stratum_core::mining_sv2::*;

// shutdown() on any clone stops a running miner, and start() then returns Ok.
#[tokio::test]
async fn test_mining_client_shutdown_returns_ok() {
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
