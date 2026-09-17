//! A Stratum V2 mining client that hashes on the CPU.
//!
//! It is a protocol testing tool, not a production miner: the hashrate is bound to the tokio
//! runtime, and the config exposes knobs for exercising a mining server, such as how many
//! standard and extended channels to open, whether to declare `REQUIRES_STANDARD_JOBS`, and a
//! multiplier for the advertised hashrate.
//!
//! One run is one connection to one mining server:
//!
//! 1. [`client::Sv2CpuMiner`] connects, runs the `SetupConnection` handshake and reads frames
//!    for the life of the connection.
//! 2. [`channel_manager::ChannelManager`] opens the configured channels, tracks channel and
//!    group state and handles every mining message.
//! 3. Each channel gets a [`miner::standard::StandardChannelMiner`] or an
//!    [`miner::extended::ExtendedChannelMiner`], hashing the channel's active job in its own
//!    task and submitting shares.
//!
//! Any protocol violation by the mining server ends the connection with an
//! [`error::Sv2CpuMinerError`]; the binary then exits with status 1.

pub mod channel_manager;
pub mod client;
pub mod config;
pub mod error;
pub mod miner;
