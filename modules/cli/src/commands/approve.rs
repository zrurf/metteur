//! Approval command handlers.

use metteur_proto::proto::daemon_client::DaemonClient;
use metteur_proto::proto::{ApprovalDecisionRequest, ResourceRequest};
use tonic::transport::Channel;

use super::*;

/// Handles `approve <request_id> <decision> [workspace|global]`.
pub(crate) async fn handle_approve(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    request_id: String,
    decision: String,
) -> anyhow::Result<Outcome> {
    let message = respond_with_receipt(
        client,
        &state.current_ws.clone().unwrap_or_default(),
        &request_id,
        &decision,
    )
    .await?;
    Ok(Outcome::Printed(message))
}

async fn query(
    client: &mut DaemonClient<Channel>,
    workspace: &str,
    value: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let response = client
        .approval_delivery(ResourceRequest {
            schema_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            workspace_path: workspace.into(),
            expected_revision: 0,
            payload_json: value.to_string(),
        })
        .await
        .map_err(status)?
        .into_inner();
    if !response.error_json.is_empty() {
        anyhow::bail!("Approval receipt query failed: {}", response.error_json)
    }
    Ok(serde_json::from_str(&response.payload_json)?)
}
/// All CLI entry points use the original immutable broker identity. Unknown
/// responses only inspect/fence that ID; they never issue another grant.
pub(crate) async fn respond_with_receipt(
    client: &mut DaemonClient<Channel>,
    workspace: &str,
    request_id: &str,
    decision: &str,
) -> anyhow::Result<String> {
    if !matches!(
        decision,
        "AllowOnce"
            | "AllowRun"
            | "AllowWorkspace"
            | "AllowGlobal"
            | "DenyOnce"
            | "DenyRun"
            | "DenyWorkspace"
            | "DenyGlobal"
    ) {
        anyhow::bail!("Invalid approval decision; the pending request was kept")
    }
    let caps = query(client, workspace, serde_json::json!({"operation":"capabilities"})).await?;
    if caps["durable_decisions"] != true || caps["fixed_target_content"] != true {
        anyhow::bail!("Update the daemon before using protected approval decisions")
    }
    let described =
        query(client, workspace, serde_json::json!({"operation":"describe","id":request_id}))
            .await?;
    if !described["receipt"].is_null() {
        let receipt = &described["receipt"];
        if receipt["phase"] == "accepted" && receipt["decision"] == decision {
            return Ok(format!("approval {request_id}: {decision} already accepted; not replayed"));
        }
        anyhow::bail!(
            "Approval {request_id} has an existing {} outcome; no new decision was issued",
            receipt["phase"]
        )
    }
    let pending = &described["pending"];
    let digest = pending["content_digest"]
        .as_str()
        .filter(|value| value.len() == 64)
        .ok_or_else(|| anyhow::anyhow!("Approval {request_id} is no longer pending"))?;
    let target = pending["target"].clone();
    let outcome = client
        .respond_approval(ApprovalDecisionRequest {
            workspace_path: workspace.into(),
            request_id: request_id.into(),
            decision: decision.into(),
            target_json: target.to_string(),
            content_digest: digest.into(),
        })
        .await;
    let checked = query(client, workspace, if outcome.is_ok() { serde_json::json!({"operation":"read","id":request_id}) } else { serde_json::json!({"operation":"read_or_retire","id":request_id,"target":target,"content_digest":digest}) }).await?;
    if checked["receipt"]["phase"] == "accepted" && checked["receipt"]["decision"] == decision {
        return Ok(format!("approval {request_id}: {decision} accepted"));
    }
    anyhow::bail!(
        "Approval {request_id} outcome is {}. Inspect this identity; do not issue a replacement grant",
        checked["receipt"]["phase"]
    )
}

/// Handles `approve-auto on|off`.
pub(crate) async fn handle_approve_auto(
    state: &mut SessionState,
    on: bool,
) -> anyhow::Result<Outcome> {
    state.auto_approve = on;
    Ok(Outcome::Printed(format!(
        "auto-approve {}",
        if on {
            "enabled"
        } else {
            "disabled"
        }
    )))
}
