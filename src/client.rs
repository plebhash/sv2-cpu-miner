use crate::config::CPU_THROTTLE_WINDOW_MS;
use crate::config::Sv2CpuMinerConfig;
use crate::handler::Sv2CpuMinerClientHandler;
use anyhow::{Result, anyhow};
use stratum_apps::network_helpers::noise_connection::Connection;
use stratum_apps::stratum_core::bitcoin::{
    CompactTarget,
    blockdata::block::{Header, Version},
    hashes::sha256d::Hash,
};
use stratum_apps::stratum_core::channels_sv2::target::u256_to_block_hash;
use stratum_apps::stratum_core::codec_sv2::MessageFrame;
use stratum_apps::stratum_core::common_messages_sv2::{Protocol, SetupConnectionOwned};
use stratum_apps::stratum_core::handlers_sv2::{
    HandleCommonMessagesFromServerOwnedAsync, HandleMiningMessagesFromServerOwnedAsync,
};
use stratum_apps::stratum_core::noise_sv2::Initiator;
use stratum_apps::stratum_core::parsers_sv2::AnyMessageOwned;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

pub type Message = AnyMessageOwned;
pub type StdFrame = MessageFrame<Message>;

#[derive(Clone)]
pub struct Sv2CpuMiner {
    config: Sv2CpuMinerConfig,
    nominal_hashrate: f32,
    cancellation_token: CancellationToken,
}

impl Sv2CpuMiner {
    pub async fn new(config: Sv2CpuMinerConfig) -> Self {
        let nominal_hashrate = measure_hashrate(config.cpu_usage_percent).await;

        Self {
            config,
            nominal_hashrate,
            cancellation_token: CancellationToken::new(),
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        let socket = TcpStream::connect(self.config.server_addr).await?;
        let initiator = Initiator::new(self.config.auth_pk.as_ref().map(|k| k.0));
        let (receiver, sender) =
            Connection::connect::<StdFrame>(socket, initiator, self.cancellation_token.clone())
                .await
                .map_err(|e| anyhow!("Failed to establish noise connection: {:?}", e))?;

        let mut handler = Sv2CpuMinerClientHandler::new(
            self.config.user_identity.clone(),
            self.nominal_hashrate,
            self.config.nominal_hashrate_multiplier,
            self.config.n_extended_channels,
            self.config.n_standard_channels,
            self.config.single_submit,
            self.config.cpu_usage_percent,
            sender.clone(),
            self.cancellation_token.clone(),
        );

        // The pool rejects OpenExtendedMiningChannel on connections that declare
        // REQUIRES_STANDARD_JOBS. So the flag is only set when no extended channels are
        // requested (keeping ungrouped per-channel NewMiningJob for standard channels);
        // with extended channels the connection runs in group mode instead.
        let flags = if self.config.n_extended_channels > 0 {
            0b000_u32
        } else {
            // REQUIRES_STANDARD_JOBS, !REQUIRES_WORK_SELECTION, !REQUIRES_VERSION_ROLLING
            0b001_u32
        };

        let setup_connection = SetupConnectionOwned {
            protocol: Protocol::MiningProtocol,
            min_version: 2,
            max_version: 2,
            flags,
            endpoint_host: self
                .config
                .server_addr
                .ip()
                .to_string()
                .try_into()
                .expect("host must fit in Str0255"),
            endpoint_port: self.config.server_addr.port(),
            vendor: "".try_into().expect("empty string is valid Str0255"),
            hardware_version: "".try_into().expect("empty string is valid Str0255"),
            firmware: "".try_into().expect("empty string is valid Str0255"),
            device_id: self
                .config
                .device_id
                .clone()
                .try_into()
                .expect("device_id must fit in Str0255"),
        };
        let frame: StdFrame = Message::Common(setup_connection.into())
            .try_into()
            .expect("SetupConnection must be serializable");
        sender
            .send(frame)
            .await
            .map_err(|e| anyhow!("Failed to send SetupConnection: {}", e))?;

        let mut incoming = receiver
            .recv()
            .await
            .map_err(|e| anyhow!("Connection closed during SetupConnection: {}", e))?;
        let header = incoming.header();
        handler
            .handle_common_message_frame_from_server(None, header, incoming.payload())
            .await
            .map_err(|e| anyhow!("SetupConnection failed: {:?}", e))?;

        handler.open_channels().await?;

        loop {
            tokio::select! {
                _ = self.cancellation_token.cancelled() => {
                    return Ok(());
                }
                frame = receiver.recv() => {
                    match frame {
                        Ok(mut frame) => {
                            let header = frame.header();
                            if let Err(e) = handler
                                .handle_mining_message_frame_from_server(None, header, frame.payload())
                                .await
                            {
                                error!("Failed to handle message from server: {:?}", e);
                            }
                        }
                        Err(_) => {
                            error!("Connection closed by server");
                            self.cancellation_token.cancel();
                            return Ok(());
                        }
                    }
                }
            }
        }
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
