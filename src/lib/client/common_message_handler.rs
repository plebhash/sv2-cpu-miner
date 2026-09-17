use super::Sv2CpuMiner;
use crate::error::Sv2CpuMinerError;
use stratum_apps::stratum_core::common_messages_sv2::{
    ChannelEndpointChangedOwned, MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED, MESSAGE_TYPE_RECONNECT,
    ReconnectOwned, SetupConnectionErrorOwned, SetupConnectionSuccessOwned,
};
use stratum_apps::stratum_core::handlers_sv2::{
    HandleCommonMessagesFromServerOwnedAsync, HandlerErrorType,
};
use stratum_apps::stratum_core::parsers_sv2::Tlv;
use tracing::{error, info};

impl HandleCommonMessagesFromServerOwnedAsync for Sv2CpuMiner {
    type Error = Sv2CpuMinerError;

    fn get_negotiated_extensions_with_server(
        &self,
        _server_id: Option<usize>,
    ) -> Result<Vec<u16>, Self::Error> {
        Ok(vec![])
    }

    async fn handle_setup_connection_success(
        &mut self,
        _server_id: Option<usize>,
        msg: SetupConnectionSuccessOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received SetupConnection.Success: {}", msg);
        Ok(())
    }

    async fn handle_setup_connection_error(
        &mut self,
        _server_id: Option<usize>,
        msg: SetupConnectionErrorOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received SetupConnection.Error: {}", msg);
        Err(Sv2CpuMinerError::SetupConnectionFailed)
    }

    async fn handle_channel_endpoint_changed(
        &mut self,
        _server_id: Option<usize>,
        _msg: ChannelEndpointChangedOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received unexpected ChannelEndpointChanged");
        Err(Sv2CpuMinerError::unexpected_message(
            0,
            MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED,
        ))
    }

    async fn handle_reconnect(
        &mut self,
        _server_id: Option<usize>,
        _msg: ReconnectOwned,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        error!("Received unexpected Reconnect");
        Err(Sv2CpuMinerError::unexpected_message(
            0,
            MESSAGE_TYPE_RECONNECT,
        ))
    }
}
