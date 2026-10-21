//! Generated Metteur protobuf types and gRPC client/server stubs.
//!
//! Shared by `metteur-daemon` (server) and `metteur-cli` (client).

pub mod proto {
    tonic::include_proto!("metteur");
}

/// Bounded transport budget shared by native daemon, web proxy and CLI.
/// The 1k-node / 10k-record checkpoint fixture is ~7 MiB; tonic's implicit
/// 4 MiB read default rejected it. Endpoint-specific smaller limits still apply.
pub const MAX_RPC_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
