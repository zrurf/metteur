//! Passive startup wake mechanism.
//!
//! In passive mode the daemon listens for a `WAKE` command on a local socket
//! before fully initializing. This module provides a cross-platform wake
//! listener: a Unix domain socket on Unix and a named pipe on Windows.

use std::path::PathBuf;

use crate::error::{DaemonError, DaemonResult};

/// The default wake socket path (Unix) or pipe name (Windows).
pub fn default_wake_path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"\\.\pipe\metteur-wake")
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir().join("metteur-wake.sock")
    }
}

/// Waits for a `WAKE` command on the wake socket.
///
/// Returns once a client sends the `WAKE` command, indicating the daemon
/// should fully start.
pub async fn wait_for_wake(path: &PathBuf) -> DaemonResult<()> {
    #[cfg(windows)]
    {
        wait_for_wake_windows(path).await
    }
    #[cfg(not(windows))]
    {
        wait_for_wake_unix(path).await
    }
}

#[cfg(windows)]
async fn wait_for_wake_windows(path: &PathBuf) -> DaemonResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ServerOptions;

    tracing::info!("waiting for wake on {}", path.display());
    loop {
        let mut server =
            ServerOptions::new().first_pipe_instance(true).create(path).map_err(DaemonError::Io)?;
        server.connect().await.map_err(DaemonError::Io)?;
        let mut buf = [0u8; 64];
        let n = server.read(&mut buf).await.map_err(DaemonError::Io)?;
        let cmd = String::from_utf8_lossy(&buf[..n]).trim().to_string();
        if cmd == "WAKE" {
            let _ = server.write_all(b"OK").await;
            return Ok(());
        }
        // Non-WAKE command: drop the instance and wait for the next client.
    }
}

#[cfg(not(windows))]
async fn wait_for_wake_unix(path: &PathBuf) -> DaemonResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    if path.exists() {
        std::fs::remove_file(path).map_err(DaemonError::Io)?;
    }
    let listener = UnixListener::bind(path).map_err(DaemonError::Io)?;
    tracing::info!("waiting for wake on {}", path.display());

    loop {
        let (mut stream, _) = listener.accept().await.map_err(DaemonError::Io)?;
        let mut buf = [0u8; 64];
        let n = stream.read(&mut buf).await.map_err(DaemonError::Io)?;
        let cmd = String::from_utf8_lossy(&buf[..n]).trim().to_string();
        if cmd == "WAKE" {
            let _ = stream.write_all(b"OK").await;
            return Ok(());
        }
    }
}
