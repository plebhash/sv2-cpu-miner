//! Hashing. Each channel gets a [`standard::StandardChannelMiner`] or an
//! [`extended::ExtendedChannelMiner`] that hashes the channel's active job in its own task and
//! submits shares. The throttle window and the start-up hashrate measurement live here too.

pub mod extended;
pub mod standard;

use stratum_apps::stratum_core::bitcoin::{
    CompactTarget,
    blockdata::block::{Header, Version},
    hashes::sha256d::Hash,
};
use stratum_apps::stratum_core::channels_sv2::target::u256_to_block_hash;
use tokio::time::Duration;
use tracing::{error, info};

/// Duration of each CPU throttling cycle in milliseconds
/// The miner will work for N% of this window, then sleep for (100-N)% of this window
pub const CPU_THROTTLE_WINDOW_MS: u64 = 100;

/// A poisoned channel lock means a task panicked while updating channel state, which is a
/// bug; mining cannot sensibly continue on that channel.
pub(crate) const LOCK_POISONED: &str = "channel lock poisoned by a panicking task";

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
