//! Explicit concierge channel; results cannot dispatch approvals or execution actions.
use super::{Outcome, SessionState, require_ws};
use metteur_proto::proto::{
    ConciergeStateRequest, SendConciergeMessageRequest, daemon_client::DaemonClient,
};
use tonic::transport::Channel;
pub async fn state(
    client: &mut DaemonClient<Channel>,
    session: &SessionState,
    run_id: String,
) -> anyhow::Result<Outcome> {
    let response = client
        .get_concierge_state(ConciergeStateRequest {
            workspace_path: require_ws(session)?,
            conversation_id: run_id.clone(),
            run_id,
        })
        .await?
        .into_inner();
    Ok(Outcome::Printed(crate::print::pretty_json(&response.state_json)))
}
pub async fn send(
    client: &mut DaemonClient<Channel>,
    session: &SessionState,
    run_id: String,
    message_id: String,
    message: String,
) -> anyhow::Result<Outcome> {
    let stream = client
        .send_concierge_message(SendConciergeMessageRequest {
            workspace_path: require_ws(session)?,
            conversation_id: run_id.clone(),
            run_id,
            message_id,
            message,
        })
        .await?
        .into_inner();
    Ok(Outcome::StartedConcierge(Box::new(stream)))
}
pub fn line(event: &metteur_proto::proto::ConciergeEvent) -> String {
    format!("[concierge {} {}] {}", event.message_id, event.kind, event.detail_json)
}
