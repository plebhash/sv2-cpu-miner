//! The connection to the mining server: TCP plus Noise, the `SetupConnection` handshake, and
//! the loop that hands every incoming frame to the [`ChannelManager`].

use crate::channel_manager::ChannelManager;
use crate::config::Sv2CpuMinerConfig;
use crate::error::Sv2CpuMinerError;
use crate::miner::measure_hashrate;
use stratum_apps::network_helpers::noise_connection::Connection;
use stratum_apps::stratum_core::common_messages_sv2::{
    MESSAGE_TYPE_SETUP_CONNECTION_ERROR, MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS, Protocol,
    SetupConnectionOwned,
};
use stratum_apps::stratum_core::handlers_sv2::{
    HandleCommonMessagesFromServerOwnedAsync, HandleMiningMessagesFromServerOwnedAsync,
    HandlerErrorType,
};
use stratum_apps::stratum_core::noise_sv2::Initiator;
use stratum_apps::utils::types::{
    InboundFrame, Message, OutboundFrame, SUPPORTED_PROTOCOL_VERSION,
};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

mod common_message_handler;

/// One connection to one mining server. Clones share the cancellation token, so a clone can
/// [`shutdown`](Self::shutdown) a running [`start`](Self::start).
#[derive(Clone)]
pub struct Sv2CpuMiner {
    config: Sv2CpuMinerConfig,
    nominal_hashrate: f32,
    cancellation_token: CancellationToken,
}

impl Sv2CpuMiner {
    /// Measures this CPU's hashrate for one second at the configured CPU usage. That figure,
    /// scaled by `nominal_hashrate_multiplier`, is what the channels advertise.
    pub async fn new(config: Sv2CpuMinerConfig) -> Self {
        let nominal_hashrate = measure_hashrate(config.cpu_usage_percent).await;

        Self {
            config,
            nominal_hashrate,
            cancellation_token: CancellationToken::new(),
        }
    }

    /// The hashrate measured when this miner was built, before `nominal_hashrate_multiplier`
    /// is applied.
    pub fn nominal_hashrate(&self) -> f32 {
        self.nominal_hashrate
    }

    /// Connects, completes the handshake, opens the configured channels and handles frames
    /// until the mining server closes the connection or [`shutdown`](Self::shutdown) is called,
    /// both of which return `Ok`. Returns an error when connecting or the handshake fails, or
    /// when the mining server violates the protocol.
    pub async fn start(&mut self) -> Result<(), Sv2CpuMinerError> {
        let socket = TcpStream::connect(self.config.server_addr).await?;
        let initiator = Initiator::new(self.config.auth_pk.as_ref().map(|k| k.0));
        let (upstream_receiver, upstream_sender) = Connection::connect::<OutboundFrame>(
            socket,
            initiator,
            self.cancellation_token.clone(),
        )
        .await?;

        self.perform_setup_connection_handshake(&upstream_sender, &upstream_receiver)
            .await?;

        // validate() rules out a config that opens no channels, so this never divides by zero
        let total_channels =
            self.config.n_standard_channels as f32 + self.config.n_extended_channels as f32;
        let mut channel_manager = ChannelManager::new(
            self.config.user_identity.clone(),
            self.nominal_hashrate / total_channels,
            self.config.single_submit,
            self.config.cpu_usage_percent,
            self.config.requires_standard_jobs,
            upstream_sender.clone(),
            self.cancellation_token.clone(),
        );

        channel_manager
            .open_channels(
                self.config.n_standard_channels,
                self.config.n_extended_channels,
                self.config.nominal_hashrate_multiplier,
            )
            .await?;

        loop {
            tokio::select! {
                _ = self.cancellation_token.cancelled() => {
                    return Ok(());
                }
                frame = upstream_receiver.recv() => {
                    match frame {
                        Ok(mut frame) => {
                            let header = frame.header();
                            // a handler error is a protocol violation by the mining server,
                            // after which the connection is not worth keeping
                            channel_manager
                                .handle_mining_message_frame_from_server(None, header, frame.payload())
                                .await?;
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

    /// Runs the SetupConnection handshake: offers this miner's parameters to the mining server
    /// and hands the reply to the common message handler, which decides whether the connection
    /// can be used.
    async fn perform_setup_connection_handshake(
        &mut self,
        upstream_sender: &async_channel::Sender<OutboundFrame>,
        upstream_receiver: &async_channel::Receiver<InboundFrame>,
    ) -> Result<(), Sv2CpuMinerError> {
        // REQUIRES_STANDARD_JOBS declares that this client cannot process extended jobs
        // (SetupConnection flags table of the Mining Protocol spec). This miner can, so the
        // flag is a user choice for exercising a mining server's per-channel NewMiningJob
        // path; validate() refuses it together with extended channels, which only ever carry
        // extended jobs.
        let flags = if self.config.requires_standard_jobs {
            // REQUIRES_STANDARD_JOBS, !REQUIRES_WORK_SELECTION, !REQUIRES_VERSION_ROLLING
            0b001_u32
        } else {
            0b000_u32
        };

        let setup_connection = SetupConnectionOwned {
            protocol: Protocol::MiningProtocol,
            min_version: SUPPORTED_PROTOCOL_VERSION,
            max_version: SUPPORTED_PROTOCOL_VERSION,
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
                .expect("device_id length checked at config load"),
        };
        let frame = OutboundFrame::from_message(Message::Common(setup_connection.into()))?;
        upstream_sender.send(frame).await?;

        let mut incoming = upstream_receiver.recv().await?;
        let header = incoming.header();
        // the reply must be SetupConnection.Success or .Error; anything else is a violation
        if header.ext_type() != 0
            || !matches!(
                header.msg_type(),
                MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS | MESSAGE_TYPE_SETUP_CONNECTION_ERROR
            )
        {
            return Err(Sv2CpuMinerError::unexpected_message(
                header.ext_type(),
                header.msg_type(),
            ));
        }
        self.handle_common_message_frame_from_server(None, header, incoming.payload())
            .await
    }

    /// Cancels the connection and every mining task, making [`start`](Self::start) return.
    pub async fn shutdown(&mut self) {
        info!("Shutting down Mining Client");
        self.cancellation_token.cancel();
    }
}
