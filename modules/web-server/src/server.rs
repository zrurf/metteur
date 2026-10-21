//! axum router that serves the webcore build and the grpc-web endpoint.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::Result;
use axum::Router;
use axum::body::Body as AxumBody;
use axum::extract::Json;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use metteur_proto::proto::daemon_client::DaemonClient;
use metteur_proto::proto::daemon_server::DaemonServer;
use std::convert::Infallible;
use tonic::transport::Channel;
use tower::{Layer, Service};

use crate::{Cli, ForwardService, sse};

/// Builds the web application router.
///
/// `POST /metteur.Daemon/{method}` carries grpc-web traffic forwarded to the
/// daemon; `POST /api/pick-directory` opens a native folder dialog on the
/// host; everything else is static content from the webcore build, with an
/// `index.html` fallback for client-side routes only (extension-less paths).
/// Missing assets return a real 404 instead of HTML, so a stale hashed
/// stylesheet (e.g. after a rebuild) never surfaces as a `text/html` CSS.
pub fn build_router(client: DaemonClient<Channel>, static_dir: PathBuf) -> Router {
    let grpc_web = tonic_web::GrpcWebLayer::new()
        .layer(DaemonServer::new(ForwardService::new(client.clone())).max_decoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES).max_encoding_message_size(metteur_proto::MAX_RPC_MESSAGE_BYTES));
    let static_dir = Arc::new(static_dir);
    Router::new()
        .route_service(
            "/metteur.Daemon/{*path}",
            axum::routing::any_service(GrpcWebAdapter(grpc_web)),
        )
        .route("/api/pick-directory", axum::routing::post(pick_directory))
        // The chat stream over server-sent events; everything else stays on
        // grpc-web, which already carries unary calls well.
        .route("/api/chat/stream", axum::routing::post(sse::chat_stream))
        .with_state(client)
        .fallback(move |uri: Uri| async move { serve_static(static_dir.clone(), uri).await })
}

/// Serves a real file when present; otherwise, for extension-less paths only,
/// falls back to `index.html` (single-page app). Missing files that carry an
/// extension — typically stale build assets — yield a plain 404.
async fn serve_static(static_dir: Arc<PathBuf>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        return serve_index(&static_dir).await;
    }
    let file = static_dir.join(path);
    if file.is_file() {
        match tokio::fs::read(&file).await {
            Ok(bytes) => Response::builder()
                .header(header::CONTENT_TYPE, mime_of(&file))
                .header(header::CACHE_CONTROL, "no-store")
                .body(AxumBody::from(bytes))
                .unwrap(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    } else if Path::new(path).extension().is_none() {
        serve_index(&static_dir).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn serve_index(static_dir: &Path) -> Response {
    match tokio::fs::read(static_dir.join("index.html")).await {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CONTENT_SECURITY_POLICY, "frame-src 'none';")
            // The HTML points at hashed assets. Do not let an already-open
            // browser keep an old entry point after a Retry fix is deployed.
            .header(header::CACHE_CONTROL, "no-store")
            .body(AxumBody::from(bytes))
            .unwrap(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Content type for a static file, derived from its extension.
fn mime_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "map" => "application/json",
        "png" => "image/png",
        "ico" => "image/vnd.microsoft.icon",
        "woff2" => "font/woff2",
        "webp" => "image/webp",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Opens a native folder picker on the host and returns the chosen absolute
/// path (`{"path": "<dir>"}`, or `null` when the user cancelled).
///
/// Browsers never expose absolute paths, so the local Web Server Client asks
/// the OS directly. On Windows a PowerShell `FolderBrowserDialog` is used.
async fn pick_directory() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    pick_local_directory().map(Json).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}

fn pick_local_directory() -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    {
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$d = New-Object System.Windows.Forms.FolderBrowserDialog
$d.Description = 'Select a workspace folder'
if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    Write-Output $d.SelectedPath
}"#;
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", script])
            .output()
            .map_err(|e| format!("failed to launch folder picker: {e}"))?;
        if !output.status.success() {
            return Err("folder picker failed".to_string());
        }
        let path =
            String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or("").trim().to_string();
        let path = if path.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(path)
        };
        Ok(serde_json::json!({ "path": path }))
    }
    #[cfg(not(windows))]
    {
        Err("directory picking is only supported on Windows".to_string())
    }
}

/// Adapts tonic-web's grpc-web service (tonic body) to axum's body type so it
/// can be mounted as an axum route service.
#[derive(Clone)]
struct GrpcWebAdapter<S>(S);

impl<S> Service<axum::http::Request<AxumBody>> for GrpcWebAdapter<S>
where
    S: Service<
            axum::http::Request<AxumBody>,
            Response = axum::http::Response<tonic::body::Body>,
            Error = Infallible,
        >,
    <S as Service<axum::http::Request<AxumBody>>>::Future: Send + 'static,
{
    type Response = axum::http::Response<AxumBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx).map_err(|never| match never {})
    }

    fn call(&mut self, req: axum::http::Request<AxumBody>) -> Self::Future {
        let fut = self.0.call(req);
        Box::pin(async move {
            let res = fut.await.map_err(|never| match never {})?;
            Ok(res.map(AxumBody::new))
        })
    }
}

/// Serves the router on the configured listen address.
pub async fn serve(cli: Cli, app: Router) -> Result<()> {
    let addr: std::net::SocketAddr = cli
        .listen_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid listen address '{}': {e}", cli.listen_addr))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    tracing::info!("web server listening on {local}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn pick_directory_route_is_registered_as_post() {
        let dir = std::env::temp_dir().join(format!("metteur-web-dist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<html>metteur</html>").unwrap();
        let client =
            tonic::transport::Channel::from_shared("http://127.0.0.1:1").unwrap().connect_lazy();
        let app = build_router(DaemonClient::new(client), dir.clone());

        // GET hits a POST-only route, proving it is mounted (and not swallowed
        // by the SPA fallback).
        let response = app
            .oneshot(Request::builder().uri("/api/pick-directory").body(AxumBody::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::METHOD_NOT_ALLOWED);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn static_routes_fall_back_to_index() {
        let dir = std::env::temp_dir().join(format!("metteur-web-dist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<html>metteur</html>").unwrap();
        std::fs::write(dir.join("asset.js"), "console.log(1)").unwrap();

        let client =
            tonic::transport::Channel::from_shared("http://127.0.0.1:1").unwrap().connect_lazy();
        let app = build_router(DaemonClient::new(client), dir.clone());

        // A real asset is served as-is.
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/asset.js").body(AxumBody::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), b"console.log(1)");

        // Unknown extension-less paths fall back to index.html (SPA routes).
        let app = app.clone();
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/chat").body(AxumBody::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), b"<html>metteur</html>");

        // A missing asset (e.g. a stale hashed stylesheet) is a real 404, not
        // an HTML page — browsers must never load CSS as `text/html`.
        let response = app
            .oneshot(
                Request::builder().uri("/assets/stale-abc123.css").body(AxumBody::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::NOT_FOUND);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
