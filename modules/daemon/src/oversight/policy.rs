//! Server-side intersection of explicit delegation, sandbox mode and provenance.
use super::{actions::Proposal, scheduler::Review};
use crate::{execution::context::ExecutionContext, sandbox::PermissionMode};
use metteur_shared::config::oversight::OversightConfig;

pub(crate) fn delegated(ctx: &ExecutionContext, review: &Review, proposal: &Proposal) -> bool {
    let Some(config) = ctx.config.as_ref().and_then(|c| c.try_read().ok()) else {
        return false;
    };
    let Ok(settings) = OversightConfig::from_config(&config) else {
        return false;
    };
    if settings.mode != "autonomous"
        || ctx.permission_mode == PermissionMode::Ask
        || PermissionMode::parse(&config.sandbox.mode) == PermissionMode::Ask
        || review.triggers.contains("request")
        || !review.source_request_ids.is_empty()
        || !proposal.source_request_ids.is_empty()
        || review.circuit_node.is_some()
        || proposal.binding["dangerous"].as_bool() == Some(true)
    {
        return false;
    }
    match proposal.kind.as_str() {
        "PauseRun" => settings.delegation.pause_run,
        "blueprint_edits" => {
            let Some(before) = proposal.binding["before"].as_array() else {
                return false;
            };
            let Some(after) = proposal.binding["after"].as_array() else {
                return false;
            };
            if before.is_empty() || before.len() != after.len() {
                return false;
            }
            before.iter().zip(after).all(|(old, new)| {
                let Some(id) = old["id"].as_str() else {
                    return false;
                };
                let Some(grant) = settings
                    .delegation
                    .blueprint_edits
                    .iter()
                    .find(|g| g.node_id.to_string() == id)
                else {
                    return false;
                };
                let (Some(a), Some(b)) = (old["data"].as_object(), new["data"].as_object()) else {
                    return false;
                };
                let mut old_shape = old.clone();
                let mut new_shape = new.clone();
                old_shape["data"] = serde_json::Value::Null;
                new_shape["data"] = serde_json::Value::Null;
                if old_shape != new_shape {
                    return false;
                }
                a.keys()
                    .chain(b.keys())
                    .filter(|key| a.get(*key) != b.get(*key))
                    .all(|key| grant.fields.contains(key))
            })
        }
        _ => false,
    }
}
