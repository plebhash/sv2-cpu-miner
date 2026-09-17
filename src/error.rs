//! Error type shared by every part of the Sv2 CPU Miner.
//!
//! One flat enum with a variant per failure source, `From` impls so `?` works at every call
//! site, and a `HandlerErrorType` impl so the `handlers_sv2` traits can use it directly. This is
//! the same shape the `sv2-apps` miner apps use for their error kinds.

use std::fmt;
use stratum_apps::config_helpers::ConfigError;
use stratum_apps::network_helpers;
use stratum_apps::stratum_core::handlers_sv2::HandlerErrorType;
use stratum_apps::stratum_core::parsers_sv2::ParserError;
use stratum_apps::utils::types::{ExtensionType, MessageType};

#[derive(Debug)]
pub enum Sv2CpuMinerError {
    /// Errors on bad `TcpStream` connection.
    Io(std::io::Error),
    /// Errors loading the config from file or environment.
    BadConfigDeserialize(ConfigError),
    /// Config values that deserialize but fail validation.
    InvalidConfig(&'static str),
    /// Error from the network helpers library.
    NetworkHelpers(network_helpers::Error),
    /// Channel sender error.
    ChannelErrorSender,
    /// Channel receiver error.
    ChannelErrorReceiver(async_channel::RecvError),
    /// Error from the message parser.
    Parser(ParserError),
    /// Received an unexpected message type.
    UnexpectedMessage(ExtensionType, MessageType),
    /// Server rejected SetupConnection.
    SetupConnectionFailed,
}

impl std::error::Error for Sv2CpuMinerError {}

impl fmt::Display for Sv2CpuMinerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Sv2CpuMinerError::*;
        match self {
            Io(e) => write!(f, "I/O error: {e}"),
            BadConfigDeserialize(e) => write!(f, "bad config: {e}"),
            InvalidConfig(msg) => write!(f, "invalid config: {msg}"),
            NetworkHelpers(e) => write!(f, "network error: {e}"),
            ChannelErrorSender => write!(f, "channel send failed: connection closed"),
            ChannelErrorReceiver(e) => write!(f, "channel receive failed: {e}"),
            Parser(e) => write!(f, "parser error: {e}"),
            UnexpectedMessage(extension_type, message_type) => write!(
                f,
                "unexpected message: extension_type {extension_type}, message_type {message_type}"
            ),
            SetupConnectionFailed => write!(f, "SetupConnection rejected by server"),
        }
    }
}

impl From<std::io::Error> for Sv2CpuMinerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ConfigError> for Sv2CpuMinerError {
    fn from(e: ConfigError) -> Self {
        Self::BadConfigDeserialize(e)
    }
}

impl From<network_helpers::Error> for Sv2CpuMinerError {
    fn from(e: network_helpers::Error) -> Self {
        Self::NetworkHelpers(e)
    }
}

impl<T> From<async_channel::SendError<T>> for Sv2CpuMinerError {
    fn from(_: async_channel::SendError<T>) -> Self {
        Self::ChannelErrorSender
    }
}

impl From<async_channel::RecvError> for Sv2CpuMinerError {
    fn from(e: async_channel::RecvError) -> Self {
        Self::ChannelErrorReceiver(e)
    }
}

impl HandlerErrorType for Sv2CpuMinerError {
    fn unexpected_message(extension_type: ExtensionType, message_type: MessageType) -> Self {
        Self::UnexpectedMessage(extension_type, message_type)
    }

    fn parse_error(error: ParserError) -> Self {
        Self::Parser(error)
    }
}
