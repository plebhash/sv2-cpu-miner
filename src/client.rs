use crate::config::CPU_THROTTLE_WINDOW_MS;
use crate::config::Sv2CpuMinerConfig;
use crate::handler::Sv2CpuMinerClientHandler;
use anyhow::{Result, anyhow};
use bitcoin::{
    CompactTarget,
    blockdata::block::{Header, Version},
    hashes::sha256d::Hash,
};
use sv2_services::client::service::Sv2ClientService;
use sv2_services::client::service::config::Sv2ClientServiceConfig;
use sv2_services::client::service::config::Sv2ClientServiceMiningConfig;
use sv2_services::client::service::event::Sv2ClientEvent;
use sv2_services::client::service::subprotocols::template_distribution::handler::NullSv2TemplateDistributionClientHandler;
use sv2_services::roles_logic_sv2::utils::u256_to_block_hash;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

#[derive(Clone)]
pub struct Sv2CpuMiner {
    sv2_client_service:
        Sv2ClientService<Sv2CpuMinerClientHandler, NullSv2TemplateDistributionClientHandler>,
    cancellation_token: CancellationToken,
}

impl Sv2CpuMiner {
    pub async fn new(config: Sv2CpuMinerConfig) -> Result<Self> {
        let service_config = Sv2ClientServiceConfig {
            min_supported_version: 2,
            max_supported_version: 2,
            endpoint_host: None,
            endpoint_port: None,
            vendor: None,
            hardware_version: None,
            firmware: None,
            device_id: Some(config.device_id),
            mining_config: Some(Sv2ClientServiceMiningConfig {
                server_addr: config.server_addr,
                auth_pk: config.auth_pk,
                // REQUIRES_VERSION_ROLLING, !REQUIRES_WORK_SELECTION, REQUIRES_STANDARD_JOBS
                setup_connection_flags: 0b001_u32,
            }),
            job_declaration_config: None,
            template_distribution_config: None,
        };

        let template_distribution_handler = NullSv2TemplateDistributionClientHandler;

        let cancellation_token = CancellationToken::new();
        let (tx, rx) = async_channel::unbounded::<Sv2ClientEvent<'static>>();

        let nominal_hashrate = measure_hashrate(config.cpu_usage_percent).await;

        let mining_handler = Sv2CpuMinerClientHandler::new(
            config.user_identity,
            nominal_hashrate,
            config.nominal_hashrate_multiplier,
            config.n_extended_channels,
            config.n_standard_channels,
            config.single_submit,
            config.cpu_usage_percent,
            tx,
            cancellation_token.clone(),
        );

        let sv2_client_service = Sv2ClientService::new_with_event_injector(
            service_config,
            mining_handler,
            template_distribution_handler,
            rx,
            cancellation_token.clone(),
        )
        .map_err(|e| anyhow!("Failed to create Sv2ClientService: {:?}", e))?;

        Ok(Self {
            sv2_client_service,
            cancellation_token,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        self.sv2_client_service
            .start()
            .await
            .map_err(|e| anyhow!("Failed to start Sv2ClientService: {:?}", e))?;

        Ok(())
    }

    pub async fn shutdown(&mut self) {
        info!("Shutting down Mining Client");
        self.cancellation_token.cancel();
    }
}

/// Measures the hashrate of this CPU for 1 second
/// Returns the hashrate in hashes per second
pub async fn measure_hashrate(cpu_usage_percent: u64) -> f32 {
    // Simple fixed values for benchmarking - we just need a valid header structure
    let version = Version::from_consensus(536870912);
    let prev_hash = [0; 32];
    let merkle_root = [0; 32];
    let bits = CompactTarget::from_consensus(545259519);

    let mut nonce = 0;
    let mut ntime = 0;
    let mut hash_count = 0u64;

    // Time-based throttling: work for cpu_usage_percent ms, then sleep for (100-cpu_usage_percent)ms in CPU_THROTTLE_WINDOW_MS windows
    let work_duration_ms = cpu_usage_percent;
    let sleep_duration_ms = CPU_THROTTLE_WINDOW_MS - cpu_usage_percent;
    let mut window_start = std::time::Instant::now();

    let start_time = std::time::Instant::now();
    let duration = std::time::Duration::from_secs(1);

    info!("Starting hashrate measurement...");

    loop {
        // Check if we've exceeded our measurement duration
        if start_time.elapsed() >= duration {
            break;
        }

        // Time-based CPU throttling
        if cpu_usage_percent < 100 {
            let elapsed_in_window = window_start.elapsed().as_millis() as u64;
            if elapsed_in_window >= work_duration_ms {
                // Time to sleep for the throttle period
                tokio::time::sleep(Duration::from_millis(sleep_duration_ms)).await;
                window_start = std::time::Instant::now(); // Reset window
            }
        }

        // Create the block header
        let header = Header {
            version,
            prev_blockhash: u256_to_block_hash(prev_hash.into()),
            merkle_root: (*Hash::from_bytes_ref(&merkle_root)).into(),
            time: ntime,
            bits,
            nonce,
        };

        // Perform the hash (this is what we're measuring)
        let _hash = header.block_hash();
        hash_count += 1;

        // Increment nonce for next iteration
        nonce = match nonce.checked_add(1) {
            Some(n) => n,
            None => {
                // Nonce overflow, increment time and reset nonce
                ntime = match ntime.checked_add(1) {
                    Some(t) => t,
                    None => {
                        error!("Both nonce and ntime overflowed during hashrate measurement");
                        break;
                    }
                };
                0
            }
        };

        // Yield to prevent blocking the runtime
        tokio::task::yield_now().await;
    }

    let elapsed_secs = start_time.elapsed().as_secs_f32();
    let hashrate = hash_count as f32 / elapsed_secs;

    info!(
        "Hashrate measurement complete... total available CPU hashrate: {} H/s",
        format_number_with_underscores(hashrate as u64)
    );

    hashrate
}

/// Formats a number with underscores for better readability
/// e.g., 1000 -> "1_000", 1000000 -> "1_000_000"
pub fn format_number_with_underscores(num: u64) -> String {
    let num_str = num.to_string();
    let mut result = String::new();
    let chars: Vec<char> = num_str.chars().collect();

    for (i, ch) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i) % 3 == 0 {
            result.push('_');
        }
        result.push(*ch);
    }

    result
}
