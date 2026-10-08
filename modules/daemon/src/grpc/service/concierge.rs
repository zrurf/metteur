//! Concierge RPCs are an isolated read/intake channel, never a chat run or approval alias.
use super::super::proto::{
    ConciergeEvent, ConciergeState, ConciergeStateRequest, SendConciergeMessageRequest,
};
use super::{DaemonService, to_status};
use crate::{
    execution::DbCheckpointSink,
    oversight::{budget, conversation, requests},
};
use metteur_shared::config::oversight::OversightConfig;
use std::path::PathBuf;
use tonic::{Request, Response, Status};
use uuid::Uuid;

impl DaemonService {
    pub(crate) async fn get_concierge_state(
        &self,
        request: Request<ConciergeStateRequest>,
    ) -> Result<Response<ConciergeState>, Status> {
        let req = request.into_inner();
        let run =
            Uuid::parse_str(&req.run_id).map_err(|_| Status::invalid_argument("invalid run id"))?;
        let conversation_id = if req.conversation_id.is_empty() {
            run
        } else {
            Uuid::parse_str(&req.conversation_id)
                .map_err(|_| Status::invalid_argument("invalid conversation id"))?
        };
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        DbCheckpointSink::load(&ws.db, run)
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found("blueprint run not found"))?;
        let config = ws.config.read().await.clone();
        let settings = OversightConfig::from_config(&config)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let active = self.state.running.read().await.get(&ws.root).is_some_and(|r| {
            r.run_id == run && !r.cancel_requested.load(std::sync::atomic::Ordering::SeqCst)
        });
        let queue = requests::load(&ws.db, run).map_err(to_status)?;
        let usage = budget::summary(&ws.db, run, &settings).map_err(to_status)?;
        let reason = if !active || queue.closed {
            "Run is read-only".into()
        } else if let Err(e) = conversation::client(&config, &self.state.llm_factory) {
            e.to_string()
        } else if usage.exhausted {
            "Oversight token budget exhausted".into()
        } else {
            String::new()
        };
        let messages: Vec<_> = conversation::load(&ws.db, run)
            .map_err(to_status)?
            .turns
            .into_iter()
            .filter(|t| t.conversation_id == conversation_id)
            .collect();
        let schedule=crate::oversight::scheduler::load(&ws.db,run).map_err(to_status)?;
        let consumer_enabled=active && schedule.as_ref().is_some_and(|s|!s.closed);
        let reports:Vec<_>=schedule.into_iter().flat_map(|s|s.reviews)
            .filter(|r|r.source_request_ids.iter().any(|id|queue.requests.iter().any(|q|q.request_id==*id && q.conversation_id==conversation_id)))
            .map(|r|crate::oversight::projection::review(&ws.db,&r,&queue))
            .collect::<crate::DaemonResult<Vec<_>>>().map_err(to_status)?;
        Ok(Response::new(ConciergeState{state_json:serde_json::json!({"run_id":run,"conversation_id":conversation_id,"read_only":!active||queue.closed,"available":reason.is_empty(),"reason":reason,"consumer_enabled":consumer_enabled,"reports":reports,"messages":messages,"requests":queue.requests,"budget":usage}).to_string()}))
    }
    pub(crate) async fn send_concierge_message(
        &self,
        request: Request<SendConciergeMessageRequest>,
    ) -> Result<
        Response<tokio_stream::wrappers::ReceiverStream<Result<ConciergeEvent, Status>>>,
        Status,
    > {
        let req = request.into_inner();
        let run =
            Uuid::parse_str(&req.run_id).map_err(|_| Status::invalid_argument("invalid run id"))?;
        let conversation_id = Uuid::parse_str(&req.conversation_id)
            .map_err(|_| Status::invalid_argument("invalid conversation id"))?;
        let id = Uuid::parse_str(&req.message_id)
            .map_err(|_| Status::invalid_argument("invalid message id"))?;
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        // A retry of a persisted message only reads its status, even after closure.
        if let Some(existing) =
            conversation::load(&ws.db, run).map_err(to_status)?.turns.iter().find(|t| t.id == id)
        {
            if existing.conversation_id != conversation_id || existing.original_text != req.message
            {
                return Err(Status::already_exists("message id bound to different content"));
            }
            tx.send(Ok(event(run, existing)))
                .await
                .map_err(|_| Status::cancelled("stream closed"))?;
            return Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)));
        }
        if !self.state.running.read().await.get(&ws.root).is_some_and(|r| {
            r.run_id == run && !r.cancel_requested.load(std::sync::atomic::Ordering::SeqCst)
        }) {
            return Err(Status::failed_precondition("no matching active blueprint run"));
        }
        let config = ws.config.read().await.clone();
        let (model_key, client) =
            conversation::client(&config, &self.state.llm_factory).map_err(to_status)?;
        let settings = OversightConfig::from_config(&config)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        if budget::summary(&ws.db, run, &settings).map_err(to_status)?.exhausted {
            return Err(Status::resource_exhausted("oversight token budget exhausted"));
        }
        if let Some(existing) = conversation::begin(&ws.db, run, conversation_id, id, &req.message)
            .map_err(to_status)?
        {
            tx.send(Ok(event(run, &existing)))
                .await
                .map_err(|_| Status::cancelled("stream closed"))?;
        } else {
            tx.send(Ok(ConciergeEvent {
                run_id: run.to_string(),
                message_id: id.to_string(),
                kind: "processing".into(),
                detail_json: "{}".into(),
            }))
            .await
            .map_err(|_| Status::cancelled("stream closed"))?;
            tokio::spawn(async move {
                let result =
                    conversation::answer(&ws.db, run, id, &config, &model_key, client.as_ref())
                        .await;
                let response = result.map(|turn| event(run, &turn)).map_err(to_status);
                let _ = tx.send(response).await;
            });
        }
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}
fn event(run: Uuid, turn: &conversation::Turn) -> ConciergeEvent {
    ConciergeEvent {
        run_id: run.to_string(),
        message_id: turn.id.to_string(),
        kind: turn.state.clone(),
        detail_json: serde_json::to_string(turn).expect("serializable turn"),
    }
}

#[cfg(test)]
#[path = "concierge_tests.rs"]
mod tests;
