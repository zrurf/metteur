//! MCP server listing command handler.

use metteur_proto::proto::RegistryRequest;
use metteur_proto::proto::daemon_client::DaemonClient;
use tonic::transport::Channel;

use super::*;
use crate::print;

/// Handles `mcp`: lists registered MCP servers.
pub(crate) async fn handle_mcp(client: &mut DaemonClient<Channel>, state: &SessionState) -> anyhow::Result<Outcome> {
    let list = client.list_mcp_servers(RegistryRequest {workspace_path:state.current_ws.clone().unwrap_or_default()}).await.map_err(status)?.into_inner();
    Ok(Outcome::Printed(print::mcp_servers(&list)))
}
