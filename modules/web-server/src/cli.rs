//! Command-line interface and daemon channel construction.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use metteur_proto::proto::daemon_client::DaemonClient;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

/// Web Server Client for the Metteur daemon.
#[derive(Debug, Parser)]
#[command(name = "metteur-web", about = "Web Server Client for the Metteur daemon")]
pub struct Cli {
    /// Address to serve the web app and grpc-web endpoint on.
    #[arg(long, default_value = "127.0.0.1:8787")]
    pub listen_addr: String,

    /// gRPC address of the daemon to proxy to.
    #[arg(long, default_value = "127.0.0.1:50051")]
    pub daemon_addr: String,

    /// Directory holding the built webcore assets.
    #[arg(long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../modules/webcore/dist"))]
    pub static_dir: PathBuf,

    /// CA certificate (PEM) used to verify the daemon; enables TLS.
    #[arg(long)]
    pub daemon_tls_ca: Option<PathBuf>,

    /// Client certificate (PEM) presented to the daemon.
    #[arg(long)]
    pub daemon_tls_cert: Option<PathBuf>,

    /// Client private key (PEM) presented to the daemon.
    #[arg(long)]
    pub daemon_tls_key: Option<PathBuf>,
}

/// Connects a gRPC client to the daemon, over plaintext or mTLS.
pub async fn connect_daemon(cli: &Cli) -> Result<DaemonClient<Channel>> {
    let channel = match &cli.daemon_tls_ca {
        Some(ca) => {
            let mut tls = ClientTlsConfig::new().ca_certificate(read_cert(ca)?);
            if let (Some(cert), Some(key)) = (&cli.daemon_tls_cert, &cli.daemon_tls_key) {
                let identity = Identity::from_pem(
                    &std::fs::read(cert).context("reading client cert")?,
                    &std::fs::read(key).context("reading client key")?,
                );
                tls = tls.identity(identity);
            }
            Channel::from_shared(format!("https://{}", cli.daemon_addr))
                .context("invalid daemon address")?
                .tls_config(tls)
                .context("invalid TLS config")?
                .connect()
                .await
                .context("connecting to daemon over TLS")?
        }
        None => Channel::from_shared(format!("http://{}", cli.daemon_addr))
            .context("invalid daemon address")?
            .connect()
            .await
            .context("connecting to daemon")?,
    };
    Ok(DaemonClient::new(channel).max_decoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES).max_encoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES))
}

fn read_cert(path: &PathBuf) -> Result<Certificate> {
    Ok(Certificate::from_pem(
        &std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
    ))
}
