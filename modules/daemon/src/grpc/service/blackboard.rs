//! Workspace-scoped read-only blackboard query; the standard RPC ACL applies.
use super::super::proto::{BlackboardProjection, GetBlackboardRequest};
use super::{DaemonService, to_status};
use crate::execution::{
    DbCheckpointSink,
    blackboard::{self, Query},
};
use crate::observability::{anon::Anonymizer, audit::AuditWriter};
use std::path::PathBuf;
use tonic::{Request, Response, Status};

impl DaemonService {
    pub(crate) async fn get_blackboard(
        &self,
        request: Request<GetBlackboardRequest>,
    ) -> Result<Response<BlackboardProjection>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let run = uuid::Uuid::parse_str(&req.run_id)
            .map_err(|_| Status::invalid_argument("invalid run id"))?;
        if req.query_json.len() > 16_384 {
            return Err(Status::invalid_argument("query too large"));
        }
        let mut query: Query = if req.query_json.trim().is_empty() {
            Query::default()
        } else {
            serde_json::from_str(&req.query_json)
                .map_err(|_| Status::invalid_argument("invalid blackboard query"))?
        };
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let checkpoint = DbCheckpointSink::load(&ws.db, run)
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found("blueprint run not found in this workspace"))?;
        // Always apply built-ins at this boundary, plus configured patterns.
        // No deanonymization map or full context is exposed to the caller.
        let config = ws.config.read().await;
        let settings = metteur_shared::config::oversight::OversightConfig::from_config(&config).map_err(|e| Status::invalid_argument(e.to_string()))?;
        let patterns = config.anonymize.extra_patterns.clone();
        drop(config);
        query.last_n = query.last_n.min(settings.blackboard.max_entries);
        let mut projection = blackboard::project(
            &run.to_string(),
            &checkpoint.view,
            &checkpoint.exec_tree,
            &query,
            &Anonymizer::new(&patterns),
        )
        .await;
        for entry in &mut projection.entries {
            if let Some(digest) = &mut entry.digest { *digest = digest.chars().take(settings.blackboard.digest_chars).collect(); }
        }
        if query.entry_id.is_some() && projection.entries.is_empty() {
            return Err(Status::not_found("blackboard entry not found"));
        }
        // Do not copy keyword text (which may itself contain a secret) to audit.
        AuditWriter::new(ws.db.clone()).record(&subject, "blackboard.query", serde_json::json!({
            "run_id": run, "returned_entries": projection.entries.len(), "evidence_lookup": query.entry_id.is_some()
        })).map_err(to_status)?;
        let mut value = serde_json::to_value(&projection)
            .map_err(|_| Status::internal("cannot serialize blackboard"))?;
        let queue = crate::oversight::requests::load(&ws.db, run).map_err(to_status)?;
        // The board exposes immutable status/evidence, not a queue mutation API.
        let requests: Vec<_> = queue.requests.iter().rev().take(1000).map(|r| serde_json::json!({
            "request_id": r.request_id, "source": r.source, "state": r.state,
            "revision": r.revision, "review_id": r.review_id, "proposals": r.proposals,
            "result_refs": r.result_refs,
        })).collect();
        value["requests"] = serde_json::json!(requests);
        value["requests_closed"] = serde_json::json!(queue.closed);
        value["requests_total"] = serde_json::json!(queue.requests.len());
        if let Some(schedule) = crate::oversight::scheduler::load(&ws.db, run).map_err(to_status)? {
            value["reviews"] = serde_json::json!(schedule.reviews.iter().map(|r| serde_json::json!({"review_id":r.review_id,"status":r.status,"source_request_ids":r.source_request_ids,"summary":r.summary,"notes":r.work.notes,"origin":"supervisor","evidence_kind":"model_opinion","verdict":r.verdict,"actual_action_refs":r.actual_action_refs})).collect::<Vec<_>>());
        }
        let projection_json = value.to_string();
        Ok(Response::new(BlackboardProjection {
            projection_json,
        }))
    }
}
