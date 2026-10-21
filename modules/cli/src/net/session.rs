//! gRPC channel setup for the CLI client (plaintext or mutual TLS).

use std::path::{Path, PathBuf};

use anyhow::Context;
use metteur_proto::proto::Empty;
use metteur_proto::proto::daemon_client::DaemonClient;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

/// PEM file paths that enable mutual TLS when provided together.
#[derive(Debug, Clone)]
pub struct TlsPaths {
    /// CA certificate used to verify the daemon.
    pub ca: PathBuf,
    /// Client certificate presented to the daemon.
    pub cert: PathBuf,
    /// Client private key matching `cert`.
    pub key: PathBuf,
}

impl TlsPaths {
    /// Combines the three optional PEM arguments into an mTLS configuration.
    ///
    /// Returns `None` when nothing is configured and an error when only a
    /// subset of the three paths is given.
    pub fn from_args(
        ca: Option<PathBuf>,
        cert: Option<PathBuf>,
        key: Option<PathBuf>,
    ) -> anyhow::Result<Option<Self>> {
        match (ca, cert, key) {
            (None, None, None) => Ok(None),
            (Some(ca), Some(cert), Some(key)) => Ok(Some(Self {
                ca,
                cert,
                key,
            })),
            _ => anyhow::bail!(
                "--tls-ca, --tls-cert and --tls-key must be provided together to enable mTLS"
            ),
        }
    }
}

/// Connects to the daemon at `addr`, optionally over mutual TLS.
///
/// Besides establishing the transport, a lightweight `ListWorkspaces` call is
/// issued so that authentication, authorization and daemon readiness are
/// verified before the caller shows a connection banner. A failure here means
/// the daemon is effectively unusable and should be treated as unreachable.
pub async fn connect(addr: &str, tls: Option<&TlsPaths>) -> anyhow::Result<DaemonClient<Channel>> {
    let endpoint = match tls {
        None => Channel::from_shared(addr.to_string()).context("invalid daemon address")?,
        Some(tls) => {
            let ca = read_pem(&tls.ca, "TLS CA certificate")?;
            let cert = read_pem(&tls.cert, "TLS client certificate")?;
            let key = read_pem(&tls.key, "TLS client private key")?;
            Channel::from_shared(tls_uri(addr))
                .context("invalid daemon address")?
                .tls_config(
                    ClientTlsConfig::new()
                        .ca_certificate(Certificate::from_pem(ca))
                        .identity(Identity::from_pem(cert, key))
                        .domain_name(host_from_addr(addr)),
                )
                .context("invalid TLS configuration")?
        }
    };
    let channel = endpoint.connect().await.context("failed to connect to daemon")?;
    let mut client = DaemonClient::new(channel).max_decoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES).max_encoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES);
    // Force the TLS handshake and ACL to resolve our identity now, so an
    // authentication failure surfaces here instead of on the first command.
    client.list_workspaces(Empty {}).await.context("preflight check failed")?;
    Ok(client)
}

/// Extracts the host part of a daemon address for TLS SNI.
///
/// Strips the scheme, path and port (e.g. `http://daemon.local:50051` ->
/// `daemon.local`). Note that direct IP addresses require the server
/// certificate to carry the matching IP SAN.
fn host_from_addr(addr: &str) -> String {
    let rest = match addr.split_once("://") {
        Some((_, r)) => r,
        None => addr,
    };
    let host = rest.split(['/', '?']).next().unwrap_or(rest);
    // IPv6 literals in brackets must keep their brackets out of the SNI name.
    if let Some(end) = host.rfind(']') {
        return host[..=end].trim_matches(['[', ']']).to_string();
    }
    host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host).to_string()
}

/// Forces the `https` scheme when mTLS is enabled.
fn tls_uri(addr: &str) -> String {
    if let Some(rest) = addr.strip_prefix("http://") {
        format!("https://{rest}")
    } else if addr.contains("://") {
        addr.to_string()
    } else {
        format!("https://{addr}")
    }
}

/// Reads a PEM file with a basic sanity check to fail fast on bad input.
fn read_pem(path: &Path, what: &str) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(path).with_context(|| format!("failed to read {what}"))?;
    if !text.starts_with("-----BEGIN ") {
        anyhow::bail!("{what} is not a PEM file: {}", path.display());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_args_require_all_three_paths() {
        assert!(TlsPaths::from_args(None, None, None).unwrap().is_none());
        let p = PathBuf::from("ca.pem");
        assert!(TlsPaths::from_args(Some(p.clone()), None, None).is_err());
        assert!(
            TlsPaths::from_args(
                Some(p.clone()),
                Some(PathBuf::from("c.pem")),
                Some(PathBuf::from("k.pem"))
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn tls_uri_upgrades_scheme() {
        assert_eq!(tls_uri("http://h:1"), "https://h:1");
        assert_eq!(tls_uri("https://h:1"), "https://h:1");
        assert_eq!(tls_uri("h:1"), "https://h:1");
    }

    #[test]
    fn sni_host_is_derived_from_addr() {
        assert_eq!(host_from_addr("http://127.0.0.1:50051"), "127.0.0.1");
        assert_eq!(host_from_addr("https://daemon.local:50051"), "daemon.local");
        assert_eq!(host_from_addr("daemon.local"), "daemon.local");
        assert_eq!(host_from_addr("https://[::1]:50051"), "::1");
    }

    #[test]
    fn read_pem_rejects_non_pem() {
        let dir = std::env::temp_dir().join(format!("metteur-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.pem");
        std::fs::write(&path, "not a pem").unwrap();
        assert!(read_pem(&path, "cert").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
