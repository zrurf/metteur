//! gRPC server assembly.

use std::sync::Arc;

use tonic::transport::Server;

use super::acl::AclLayer;
use super::proto::daemon_server::DaemonServer;
use super::service::{AppState, DaemonService};
use crate::error::{DaemonError, DaemonResult};
use crate::tls::DaemonTlsConfig;

/// Builds and serves the gRPC server on the given address.
///
/// When `tls` is provided, mutual TLS is required and the ACL layer resolves
/// subjects from client certificates; otherwise the subject is `local`.
pub async fn serve(
    addr: std::net::SocketAddr,
    state: Arc<AppState>,
    tls: Option<DaemonTlsConfig>,
) -> DaemonResult<()> {
    let tls_enabled = tls.is_some();
    let builder = match tls {
        Some(config) => Server::builder()
            .tls_config(config.server)
            .map_err(|e| DaemonError::Tls(e.to_string()))?,
        None => Server::builder(),
    };

    builder
        .layer(AclLayer::new(state.acl_store.clone(), tls_enabled))
        .add_service(DaemonServer::new(DaemonService::new(state)).max_decoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES).max_encoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES))
        .serve(addr)
        .await
        .map_err(|e| DaemonError::Internal(format!("gRPC server error: {e}")))?;
    Ok(())
}
