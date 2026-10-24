//! Chat command handlers: send/abort/sessions/clear.

use metteur_proto::proto::daemon_client::DaemonClient;
use metteur_proto::proto::{DeleteChatSessionRequest, ListChatSessionsRequest, SendChatRequest};
use tonic::transport::Channel;

use super::*;
use crate::print;

/// Handles `chat send <text...>`: starts a chat turn and streams deltas.
///
/// Without an explicit session id the turn resumes the workspace's newest
/// thread (creating one when none exists), so repeated sends continue the
/// same conversation.
pub(crate) async fn handle_send(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    text: String,
    session_id: Option<String>,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let session_id = match session_id {
        Some(id) => Some(id),
        None => {
            let list = client
                .list_chat_sessions(ListChatSessionsRequest {
                    workspace_path: ws.clone(),
                })
                .await
                .map_err(status)?
                .into_inner();
            list.sessions.into_iter().next().map(|s| s.session_id)
        }
    };
    let stream = client
        .send_chat(SendChatRequest {
            workspace_path: ws,
            message: text.clone(),
            history_json: String::new(),
            options_json: String::new(),
            session_id: session_id.unwrap_or_default(),
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(Outcome::StartedChat(Box::new(ChatStart {
        stream,
        label: format!("chat {}", truncate(&text, 40)),
    })))
}

/// Handles `chat abort`: aborts the running chat turn.
pub(crate) async fn handle_abort(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let target = live_target(client, &ws, true).await?;
    if target.kind != "chat" {
        anyhow::bail!("The foreground owner is not a chat; abort was not redirected");
    }
    Ok(Outcome::Printed(super::live::workflow_control(client, &ws, &target, "stop", None).await?))
}

/// Handles `chat sessions`: lists chat threads of the workspace.
pub(crate) async fn handle_sessions(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let list = client
        .list_chat_sessions(ListChatSessionsRequest {
            workspace_path: ws,
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(Outcome::Printed(print::chat_sessions(&list)))
}

/// Handles `chat clear [session_id]`: deletes a thread (default: latest).
pub(crate) async fn handle_clear(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    session_id: Option<String>,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    client
        .delete_chat_session(DeleteChatSessionRequest {
            workspace_path: ws,
            session_id: session_id.unwrap_or_default(),
        })
        .await
        .map_err(status)?;
    Ok(Outcome::Printed("chat session cleared".to_string()))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}...", text.chars().take(max).collect::<String>())
}
