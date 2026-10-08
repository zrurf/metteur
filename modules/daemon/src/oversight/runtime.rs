//! A run-scoped lifetime; no workspace polling task survives execution.
use super::scheduler;
use crate::{execution::context::ExecutionContext, storage::persistence::Db};
use metteur_shared::config::oversight::OversightConfig;
use uuid::Uuid;

pub struct Runtime {
    db: Db,
    run: Uuid,
    timer: Option<tokio::task::JoinHandle<()>>,
    worker: tokio::task::JoinHandle<()>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.worker.abort();
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        if let Err(error) = scheduler::close(&self.db, self.run) {
            tracing::warn!(%error,"could not close review scheduler");
        }
    }
}
pub async fn start(ctx: &ExecutionContext) -> Option<Runtime> {
    let db = ctx.workspace_db.as_ref()?;
    if ctx.run_id.is_nil() {
        return None;
    }
    let config = match &ctx.config {
        Some(config) => config.read().await.clone(),
        None => Default::default(),
    };
    let settings = match OversightConfig::from_config(&config) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error,"review settings unavailable");
            return None;
        }
    };
    if let Err(error) = scheduler::initialize(db, ctx.run_id, settings.clone()) {
        tracing::warn!(%error,"review scheduler unavailable");
        return None;
    }
    let timer = if settings.triggers.interval_ms > 0 {
        let db = db.clone();
        let run = ctx.run_id;
        Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(settings.triggers.interval_ms))
                    .await;
                if scheduler::trigger(&db, run, "interval", scheduler::now()).is_err() {
                    break;
                }
            }
        }))
    } else {
        None
    };
    let mut actions = ctx.child_nested();
    let worker = {
        let db = db.clone();
        let run = ctx.run_id;
        let factory = ctx.llm_factory.clone();
        let events = ctx.events.clone();
        let broker = ctx.approvals.clone();
        tokio::spawn(async move {
            let process = async {
                loop {
                    if actions.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
                        if let Err(error) = scheduler::close(&db, run) {
                            tracing::warn!(%error, "Review closure unavailable");
                        }
                        break;
                    }
                    let notified = db.oversight_notify.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    let Ok(Some(state)) = scheduler::load(&db, run) else {
                        break;
                    };
                    if state.closed {
                        break;
                    }
                    let due = scheduler::due_at(&state);
                    if due.is_some_and(|at| at <= scheduler::now()) {
                        let Ok(Some(review)) = scheduler::claim(&db, run, scheduler::now()) else {
                            break;
                        };
                        emit(&events, &review);
                        let config = match &actions.config {
                            Some(current) => current.read().await.clone(),
                            None => config.clone(),
                        };
                        let result=match super::review::client(&config,&factory) {
                        Ok((key,client))=>super::review::evaluate_with_actions(&db,&review,&config,&key,client.as_ref(), Some(&mut actions)).await,
                        Err(_)=>scheduler::finish_with_diagnostic(&db,run,review.review_id,scheduler::Outcome{status:scheduler::Status::Failed,summary:"Supervisor model is unavailable; configure oversight.model or the workspace default model.".into(),verdict:None,notes:vec![]}, Some(super::diagnostic::Diagnostic::new(super::diagnostic::Category::Configuration,super::diagnostic::Stage::Configuration,run,review.review_id))),
                    };
                        match result {
                            Ok(review) => emit(&events, &review),
                            Err(error) => {
                                tracing::warn!(%error,"Review completion unavailable");
                            }
                        }
                        continue;
                    }
                    if let Some(at) = due {
                        tokio::select! { _=&mut notified=>{}, _=tokio::time::sleep(std::time::Duration::from_millis(at.saturating_sub(scheduler::now())))=>{} }
                    } else {
                        notified.await;
                    }
                }
            };
            if let Some(broker) = broker {
                tokio::select! {_ = process => {}, _ = broker.closed() => {if let Err(error)=scheduler::close(&db,run) {tracing::warn!(%error,"Review closure unavailable");}}}
            } else {
                process.await;
            }
        })
    };
    Some(Runtime {
        db: db.clone(),
        run: ctx.run_id,
        timer,
        worker,
    })
}

fn emit(
    events: &Option<tokio::sync::mpsc::UnboundedSender<crate::execution::ExecutionEvent>>,
    review: &scheduler::Review,
) {
    if let Some(events) = events {
        let _=events.send(crate::execution::ExecutionEvent::Oversight{review_id:review.review_id.to_string(),detail:serde_json::json!({"review_id":review.review_id,"status":review.status,"verdict":review.verdict}).to_string()});
    }
}
