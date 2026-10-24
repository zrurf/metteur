//! Server-sent events bridge for the ReAct chat stream.
//!
//! The daemon exposes chat as a gRPC server stream, which only a gRPC client
//! can consume and cannot be inspected with `curl`. This module re-publishes
//! the same events as `text/event-stream`, which the web client consumes over
//! plain HTTP and any operator can watch from a terminal.
//!
//! Every event carries the daemon's `kind` inside the JSON payload rather than
//! in the SSE `event:` field: one dispatch path on the client, and a new event
//! kind never changes the framing.

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use metteur_proto::proto::daemon_client::DaemonClient;
use metteur_proto::proto::{ChatEvent, SendChatRequest};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Code;
use tonic::transport::Channel;

/// How often a comment line is sent while nothing happens.
///
/// A long tool call produces no events for minutes; without keep-alive an idle
/// proxy would close the connection and the client would read it as a stall.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// The connected daemon, shared by every HTTP handler.
pub type SharedClient = DaemonClient<Channel>;

/// Request body of `POST /api/chat/stream`.
///
/// Mirrors the `SendChat` request fields; the three `*_json` fields stay raw
/// strings so this layer never needs to know the chat option schema.
#[derive(Debug, Deserialize)]
pub struct ChatStreamRequest {
    /// Workspace the conversation belongs to.
    pub workspace_path: String,
    /// The user's turn.
    pub message: String,
    /// Conversation history as `[{role, content}]`.
    #[serde(default)]
    pub history_json: String,
    /// Per-turn generation options.
    #[serde(default)]
    pub options_json: String,
    /// Session to continue; empty starts a new thread.
    #[serde(default)]
    pub session_id: String,
}

/// Streams one chat turn as server-sent events.
pub async fn chat_stream(
    State(mut client): State<SharedClient>,
    Json(body): Json<ChatStreamRequest>,
) -> Response {
    let request = SendChatRequest {
        workspace_path: body.workspace_path,
        message: body.message,
        history_json: body.history_json,
        options_json: body.options_json,
        session_id: body.session_id,
    };
    // Failures before the stream starts become real HTTP errors, so the client
    // can tell "the request never reached the daemon" from "the stream broke".
    let mut stream = match client.send_chat(request).await {
        Ok(response) => response.into_inner(),
        Err(status) => return http_error(status.code(), status.message()),
    };

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        loop {
            let item = tokio::select! { _ = tx.closed() => break, item = stream.next() => item };
            let Some(item) = item else {
                break;
            };
            let event = match item {
                Ok(chat) => Ok(to_event(&chat)),
                Err(status) => Ok(terminal_event(status.message())),
            };
            if tx.send(event).await.is_err() {
                // The client is gone; dropping the daemon stream cancels the
                // turn on the daemon side.
                break;
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response()
}

/// Renders one daemon event as an SSE message.
fn to_event(chat: &ChatEvent) -> Event {
    let payload = serde_json::json!({
        "kind": chat.kind,
        "content": chat.content,
        "detail_json": chat.detail_json,
    });
    Event::default().data(payload.to_string())
}

/// Renders a transport failure as the terminal event of the stream.
///
/// The chat protocol already has an `error` kind, so a mid-stream failure looks
/// like any other turn failure to the client instead of a truncated body.
fn terminal_event(message: &str) -> Event {
    let payload = serde_json::json!({ "kind": "error", "content": message, "detail_json": "" });
    Event::default().data(payload.to_string())
}

/// Maps a gRPC status onto the closest HTTP status.
fn http_error(code: Code, message: &str) -> Response {
    let status = match code {
        Code::InvalidArgument => StatusCode::BAD_REQUEST,
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::AlreadyExists => StatusCode::CONFLICT,
        Code::PermissionDenied => StatusCode::FORBIDDEN,
        Code::Unauthenticated => StatusCode::UNAUTHORIZED,
        Code::FailedPrecondition => StatusCode::PRECONDITION_FAILED,
        Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Code::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, message.to_string()).into_response()
}
