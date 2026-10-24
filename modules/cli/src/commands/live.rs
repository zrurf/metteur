//! Resolve a concrete control target, with no latest-owner fallback.
use super::status;
use metteur_proto::proto::daemon_client::DaemonClient;
use tonic::transport::Channel;

#[derive(Clone)]
pub(crate) struct LiveTarget {
    pub kind: String,
    pub run: String,
    pub session: String,
    pub turn: String,
}
pub(crate) async fn workflow_control(
    client: &mut DaemonClient<Channel>,
    workspace: &str,
    target: &LiveTarget,
    action: &str,
    body: Option<&str>,
) -> anyhow::Result<String> {
    use metteur_proto::proto::{
        AbortChatRequest, CancelRequest, InterruptRequest, PauseRequest, ResumeRequest,
    };
    let id = uuid::Uuid::new_v4();
    let target_json = if target.kind == "blueprint" {
        serde_json::json!({"kind":"blueprint","run":target.run})
    } else {
        serde_json::json!({"kind":"chat","session":target.session,"turn":target.turn})
    };
    let query = |value| serde_json::json!({"operation":"workflow","action":value});
    let mut revision = 0;
    if action == "stop" {
        let caps =
            delivery_query(client, workspace, serde_json::json!({"operation":"capabilities"}))
                .await?;
        if caps["targeted_controls"] != true {
            anyhow::bail!("Host cannot verify the concrete Stop target");
        }
    }
    if action != "stop" {
        let caps = delivery_query(
            client,
            workspace,
            query(serde_json::json!({"operation":"capabilities"})),
        )
        .await?;
        if caps["durable_controls"] != true {
            anyhow::bail!("Host does not support workflow receipts; input was not sent");
        }
        let described = delivery_query(
            client,
            workspace,
            query(serde_json::json!({"operation":"describe","target":target_json})),
        )
        .await?;
        revision = described["revision"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Missing control revision"))?;
        let prepared=delivery_query(client,workspace,query(serde_json::json!({"operation":"prepare","identity":{"id":id,"revision":revision},"target":target_json,"action":action,"body":body}))).await?;
        if prepared["receipt"]["phase"] != "prepared" {
            anyhow::bail!("Inspect existing receipt {id}; request was not repeated");
        }
    }
    eprintln!("workflow receipt: {id}");
    let receipt_json = serde_json::json!({"id":id,"revision":revision}).to_string();
    let sent = match (target.kind.as_str(), action) {
        ("blueprint", "stop") => client
            .cancel_execution(CancelRequest {
                workspace_path: workspace.into(),
                run_id: target.run.clone(),
                receipt_json,
            })
            .await
            .map(|_| ()),
        ("blueprint", "pause") => client
            .pause_execution(PauseRequest {
                workspace_path: workspace.into(),
                run_id: target.run.clone(),
                receipt_json,
            })
            .await
            .map(|_| ()),
        ("blueprint", "resume") => client
            .resume_execution(ResumeRequest {
                workspace_path: workspace.into(),
                run_id: target.run.clone(),
                receipt_json,
            })
            .await
            .map(|_| ()),
        ("chat", "stop") => client
            .abort_chat(AbortChatRequest {
                workspace_path: workspace.into(),
                session_id: target.session.clone(),
                turn_id: target.turn.clone(),
                receipt_json,
            })
            .await
            .map(|_| ()),
        ("blueprint", "temporary_input") => client
            .send_interrupt(InterruptRequest {
                workspace_path: workspace.into(),
                target_kind: "blueprint".into(),
                run_id: target.run.clone(),
                priority: "Urgent".into(),
                message: body.unwrap_or_default().into(),
                receipt_json,
                ..Default::default()
            })
            .await
            .map(|_| ()),
        _ => anyhow::bail!("Unsupported control target"),
    };
    let checked=delivery_query(client,workspace,query(serde_json::json!({"operation":"fence","id":id,"target":target_json,"action":action,"body":body}))).await.map_err(|e|anyhow::anyhow!("Outcome unconfirmed ({id}); query its original workflow receipt before deciding again. {e}"))?;
    let receipt = &checked["receipt"];
    let phase = receipt["phase"].as_str().unwrap_or("unknown");
    if receipt["id"] != id.to_string()
        || receipt["target"]["kind"] != target.kind
        || receipt["operation"] != action
        || receipt["body"] != serde_json::json!(body)
        || (target.kind == "blueprint" && receipt["target"]["run"] != target.run)
        || (target.kind == "chat"
            && (receipt["target"]["session"] != target.session
                || receipt["target"]["turn"] != target.turn))
    {
        anyhow::bail!("Receipt identity could not be verified ({id})");
    }
    if phase != "accepted"
        && !(action == "temporary_input"
            && ["queued", "claimed", "applied", "unconsumed", "unknown"].contains(&phase))
    {
        anyhow::bail!(
            "Control/input receipt {id}: {phase}. {}",
            sent.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }
    Ok(format!("{action} received ({id}): {phase}; execution status remains authoritative"))
}
pub(crate) async fn delivery_query(
    client: &mut DaemonClient<Channel>,
    workspace: &str,
    payload: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let result = client
        .chat_delivery(metteur_proto::proto::ResourceRequest {
            workspace_path: workspace.into(),
            schema_version: 1,
            payload_json: payload.to_string(),
            ..Default::default()
        })
        .await
        .map_err(status)?
        .into_inner();
    if !result.error_json.is_empty() {
        anyhow::bail!("Input query failed: {}", result.error_json);
    }
    Ok(serde_json::from_str(&result.payload_json)?)
}
pub(crate) async fn reliable_input(
    client: &mut DaemonClient<Channel>,
    workspace: &str,
    target: &LiveTarget,
    mode: &str,
    message: &str,
) -> anyhow::Result<String> {
    let caps =
        delivery_query(client, workspace, serde_json::json!({"operation":"capabilities"})).await?;
    if caps["durable_queue"] != true || caps["durable_queue_channel"] != "chat" {
        anyhow::bail!("Host cannot reliably receive live chat input; input was not sent");
    }
    let id = uuid::Uuid::new_v4();
    eprintln!("input receipt: {id}");
    let submitted=delivery_query(client,workspace,serde_json::json!({"operation":"live_submit","id":id,"session":target.session,"turn":target.turn,"mode":mode,"message":message})).await;
    let receipt = match submitted {
        Ok(receipt) => receipt,
        Err(error) => {
            let unknown = || {
                anyhow::anyhow!(
                    "Input outcome unconfirmed ({id}); read its chatDelivery receipt before submitting again. {error}"
                )
            };
            let mut data =
                delivery_query(client, workspace, serde_json::json!({"operation":"read","id":id}))
                    .await
                    .map_err(|_| unknown())?;
            if data["receipt"].is_null() {
                data = delivery_query(
                    client,
                    workspace,
                    serde_json::json!({"operation":"retire","id":id}),
                )
                .await
                .map_err(|_| unknown())?;
            }
            data["receipt"].clone()
        }
    };
    if receipt["phase"] == "retired" {
        anyhow::bail!("Input was not received ({id}); retain the original message");
    }
    if receipt["id"] != id.to_string()
        || receipt["session"] != target.session
        || receipt["turn"] != target.turn
        || receipt["live"]["mode"] != mode
        || !receipt["phase"].as_str().is_some_and(|phase| {
            ["live_queued", "live_claimed", "live_applied", "live_unconsumed", "live_unknown"]
                .contains(&phase)
        })
    {
        anyhow::bail!("Input receipt could not be verified ({id}); do not resubmit blindly");
    }
    Ok(format!("input received ({id}): {}", receipt["phase"].as_str().unwrap_or_default()))
}
/// Resolve once; later RPCs carry this identity and cannot follow a new owner.
pub(crate) async fn live_target(
    client: &mut DaemonClient<Channel>,
    workspace: &str,
    require_targets: bool,
) -> anyhow::Result<LiveTarget> {
    if require_targets {
        let result = client
            .chat_delivery(metteur_proto::proto::ResourceRequest {
                workspace_path: workspace.into(),
                schema_version: 1,
                payload_json: r#"{"operation":"capabilities"}"#.into(),
                ..Default::default()
            })
            .await
            .map_err(status)?
            .into_inner();
        if !result.error_json.is_empty() {
            anyhow::bail!("Live control capability unavailable: {}", result.error_json);
        }
        let data: serde_json::Value = serde_json::from_str(&result.payload_json)?;
        if data["targeted_controls"] != true {
            anyhow::bail!("Host does not verify concrete live control targets");
        }
    }
    let result = client
        .chat_delivery(metteur_proto::proto::ResourceRequest {
            workspace_path: workspace.into(),
            schema_version: 1,
            payload_json: r#"{"operation":"control_target"}"#.into(),
            ..Default::default()
        })
        .await
        .map_err(status)?
        .into_inner();
    if !result.error_json.is_empty() {
        anyhow::bail!("Live target unavailable: {}", result.error_json);
    }
    let data: serde_json::Value = serde_json::from_str(&result.payload_json)?;
    let field = |name: &str| data[name].as_str().unwrap_or_default().to_owned();
    let target = LiveTarget {
        kind: field("state"),
        run: field("run"),
        session: field("session"),
        turn: field("turn"),
    };
    if !(target.kind == "blueprint" && !target.run.is_empty()
        || target.kind == "chat" && !target.session.is_empty() && !target.turn.is_empty())
    {
        anyhow::bail!("No confirmed live target; no control was sent");
    }
    Ok(target)
}
