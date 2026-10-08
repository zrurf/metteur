//! Approved proposals commit only at an interpreter checkpoint boundary.
//! Unfinished intents deliberately block admission; guessing after a crash could
//! combine a new graph with a queue belonging to the old graph.
use metteur_shared::{Blueprint, NodeId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{DaemonError, DaemonResult};
use crate::execution::checkpoint::{
    CheckpointSink, DbCheckpointSink, ExecutionCheckpoint, RunStatus,
};
use crate::execution::context::ExecutionContext;
use crate::oversight::diagnostic::{self, Category, Stage};
use crate::storage::{
    blueprint_files,
    persistence::{Db, cf},
    versioning::VersionRef,
};

pub(crate) const INTENT_PREFIX: &[u8] = b"blueprint-apply:";

#[derive(Default)]
pub(crate) struct ApplyState {
    pub version: Option<VersionRef>,
    pending: Option<Proposal>,
    blocked: bool,
}

impl ApplyState {
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// Chosen by the server entry point, never deserialized from model arguments.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum Source {
    ModelTool,
    Supervisor {
        review_id: Uuid,
    },
    Circuit {
        failing_node: NodeId,
    },
}

#[derive(Clone, Serialize, Deserialize)]
struct Proposal {
    id: Uuid,
    run_id: Uuid,
    source: Source,
    base: VersionRef,
    state_digest: String,
    edits_digest: String,
    edits: serde_json::Value,
    affected: Vec<NodeId>,
    before: Blueprint,
    after: Blueprint,
    file: Vec<u8>,
    summary: String,
}

#[derive(Serialize, Deserialize)]
struct ApplyIntent {
    proposal: Proposal,
    checkpoint: ExecutionCheckpoint,
    after_version: Option<VersionRef>,
    committed: bool,
    #[serde(default)]
    abandoned: bool,
    #[serde(default)]
    recovery_note: String,
}

/// A result link is evidence of this exact committed application, never the
/// current file binding (which may have advanced since the proposal).
pub(crate) fn result_version(db: &Db, run: Uuid, review: Uuid, id: Uuid) -> DaemonResult<Option<VersionRef>> {
    let Some(bytes) = db.get(cf::EXECUTION_STATE, format!("blueprint-apply:{id}").as_bytes())? else { return Ok(None) };
    let intent: ApplyIntent = serde_json::from_slice(&bytes)
        .map_err(|_| DaemonError::Persistence("Invalid blueprint application record".into()))?;
    Ok((intent.committed && !intent.abandoned && intent.proposal.id == id
        && intent.proposal.run_id == run
        && matches!(intent.proposal.source, Source::Supervisor { review_id } if review_id == review))
        .then_some(intent.after_version).flatten())
}

fn encode(value: &impl Serialize) -> DaemonResult<Vec<u8>> {
    // Canonical map ordering also covers HashMaps in checkpoints.
    serde_json::to_value(value)
        .and_then(|v| serde_json::to_vec(&v))
        .map_err(|e| DaemonError::Serialization(e.to_string()))
}
fn digest(value: &impl Serialize) -> DaemonResult<String> {
    Ok(Sha256::digest(encode(value)?).iter().map(|b| format!("{b:02x}")).collect())
}
fn rejected(message: &str) -> DaemonError {
    DaemonError::Execution(message.into())
}
fn db(ctx: &ExecutionContext) -> DaemonResult<&Db> {
    ctx.workspace_db.as_ref().ok_or_else(|| rejected("replan requires a workspace database"))
}
fn checkpoint(ctx: &ExecutionContext) -> DaemonResult<ExecutionCheckpoint> {
    DbCheckpointSink::load(db(ctx)?, ctx.run_id)?
        .filter(|c| c.status == RunStatus::Running)
        .ok_or_else(|| rejected("replan requires an active persisted run"))
}

pub(crate) fn attach(
    ctx: &ExecutionContext,
    resumed: Option<&ExecutionCheckpoint>,
) -> DaemonResult<()> {
    let Some(db) = &ctx.workspace_db else {
        return Ok(());
    };
    ensure_resolved(db)?;
    let Some(graph) = &ctx.blueprint else {
        return Ok(());
    };
    let graph = graph.read();
    let bound = blueprint_files::binding(db, graph.id)?;
    if let Some(base) = &bound {
        let versions = ctx
            .version_manager
            .as_ref()
            .ok_or_else(|| rejected("bound blueprint requires Version Flow"))?;
        versions.verify_blueprint(base)?;
        if blueprint_files::decode(&std::fs::read(versions.blueprint_path(&base.blueprint_uri)?)?)?
            != *graph
        {
            return Err(rejected(
                "blueprint file differs from execution graph; save explicitly first",
            ));
        }
    }
    if let Some(cp) = resumed {
        let identity = |v: &Option<VersionRef>| {
            v.as_ref().map(|v| (v.blueprint_uri.clone(), v.blob_hash.clone()))
        };
        if identity(&cp.blueprint_version) != identity(&bound) {
            return Err(rejected(
                "blueprint changed since checkpoint; start a new run, automatic resume refused",
            ));
        }
    }
    ctx.blueprint_apply.lock().version = bound;
    Ok(())
}

/// External edits are recorded by Version Flow, but cannot silently replace the
/// plan used by a running node/tool loop.
pub(crate) fn verify_current(ctx: &ExecutionContext) -> DaemonResult<()> {
    let state = ctx.blueprint_apply.lock();
    if state.blocked {
        return Err(DaemonError::Persistence("blueprint application requires recovery".into()));
    }
    if let Some(version) = &state.version {
        ctx.version_manager
            .as_ref()
            .ok_or_else(|| rejected("Version Flow unavailable"))?
            .verify_blueprint(version)?;
    }
    Ok(())
}

pub fn ensure_resolved(db: &Db) -> DaemonResult<()> {
    for (key, bytes) in db.scan(cf::EXECUTION_STATE)? {
        if key.starts_with(INTENT_PREFIX) {
            let intent: ApplyIntent = serde_json::from_slice(&bytes).map_err(|e| {
                DaemonError::Persistence(format!("invalid blueprint apply intent: {e}"))
            })?;
            if !intent.committed && !intent.abandoned {
                return Err(DaemonError::Persistence(format!(
                    "blueprint apply {} is incomplete; manual recovery required before execution",
                    intent.proposal.id
                )));
            }
        }
    }
    Ok(())
}

fn verify(ctx: &ExecutionContext, proposal: &Proposal) -> DaemonResult<()> {
    if matches!(proposal.source, Source::Supervisor { .. }) {
        crate::oversight::actions::enabled(ctx)?;
    }
    if ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst)
        || ctx.approvals.as_ref().is_none_or(|b| b.is_closed())
    {
        return Err(rejected("replan run closed or cancelled"));
    }
    let versions =
        ctx.version_manager.as_ref().ok_or_else(|| rejected("replan requires Version Flow"))?;
    ensure_resolved(db(ctx)?)?;
    versions.verify_blueprint(&proposal.base)?;
    if ctx.run_id != proposal.run_id
        || *ctx.blueprint.as_ref().ok_or_else(|| rejected("no shared blueprint"))?.read()
            != proposal.before
        || digest(&checkpoint(ctx)?)? != proposal.state_digest
    {
        return Err(rejected(
            "proposal is stale: blueprint or execution state changed; propose again",
        ));
    }
    Ok(())
}

pub(crate) async fn approve(
    ctx: &mut ExecutionContext,
    summary: &str,
    edits: &serde_json::Value,
    source: Source,
) -> DaemonResult<String> {
    let argument_error = |error| if matches!(source, Source::Supervisor { .. }) {
        diagnostic::error(Category::ToolArguments, Stage::ToolDispatch)
    } else { error };
    let version_error = |error| if matches!(source, Source::Supervisor { .. }) {
        let (category, stage) = diagnostic::classify(&error, Stage::VersionCheck);
        diagnostic::error(category, stage)
    } else { error };
    if ctx.blueprint_apply.lock().pending.is_some() {
        return Err(rejected("a replan is already awaiting the next node boundary"));
    }
    let before =
        ctx.blueprint.as_ref().ok_or_else(|| rejected("no shared blueprint"))?.read().clone();
    let versions =
        ctx.version_manager.as_ref().ok_or_else(|| rejected("replan requires Version Flow"))?;
    let base = ctx.blueprint_apply.lock().version.clone().ok_or_else(|| {
        rejected("legacy blueprint must be explicitly saved to a file before modification")
    })?;
    versions.verify_blueprint(&base).map_err(version_error)?;
    let file = std::fs::read(versions.blueprint_path(&base.blueprint_uri)?)?;
    if blueprint_files::decode(&file).map_err(version_error)? != before {
        return Err(version_error(rejected("executing graph differs from its authoritative file")));
    }
    let state = checkpoint(ctx)?;
    if state.blueprint_id != before.id || state.blueprint_version.as_ref() != Some(&base) {
        return Err(version_error(rejected(
            "run has no matching blueprint version; start a new run after saving",
        )));
    }
    let mut after = before.clone();
    let changed = super::apply_edits(&mut after, edits).map_err(DaemonError::Execution).map_err(argument_error)?;
    let report = metteur_shared::model::validate::validate_with_catalog(
        &after,
        &ctx.registry.node_signatures(),
    );
    if !report.is_ok() {
        return Err(argument_error(rejected(&format!("invalid proposal: {:?}", report.errors))));
    }
    let affected: Vec<_> =
        before.nodes.iter().zip(&after.nodes).filter(|(a, b)| a != b).map(|(n, _)| n.id).collect();
    if affected.is_empty() {
        return Err(rejected("proposal makes no changes"));
    }
    for id in &affected {
        let circuit_retry = match source {
            Source::Supervisor {
                review_id,
            } => {
                crate::oversight::scheduler::load(db(ctx)?, ctx.run_id)?.is_some_and(|s| {
                    !s.closed
                        && s.reviews.iter().any(|r| {
                            r.review_id == review_id
                                && r.circuit_node == Some(*id)
                                && r.status == crate::oversight::scheduler::Status::Running
                        })
                }) && state.in_flight == Some(*id)
                    && state.circuit_failures > 0
            }
            _ => false,
        };
        if !circuit_retry && (state.executed.contains(id) || state.in_flight == Some(*id)) {
            return Err(rejected(
                "proposal changes an active or completed node; structural edits and reruns require a separate workflow",
            ));
        }
    }
    let proposal = Proposal {
        id: Uuid::new_v4(),
        run_id: ctx.run_id,
        source,
        base,
        state_digest: digest(&state)?,
        edits_digest: digest(edits)?,
        edits: edits.clone(),
        affected,
        file: blueprint_files::encode_changes(&file, &after)?,
        before,
        after,
        summary: changed.clone(),
    };
    verify(ctx, &proposal)?;
    let mut oversight = match source {
        Source::Supervisor {
            review_id,
        } => Some(crate::oversight::actions::register(
            ctx,
            review_id,
            proposal.id,
            "blueprint_edits",
            serde_json::json!({
                "request_type":"replan_proposal","tool":"ProposeBlueprintEdits","source":"supervisor","retry_node": crate::oversight::scheduler::load(db(ctx)?,ctx.run_id)?.and_then(|s|s.reviews.into_iter().find(|r|r.review_id==review_id)).and_then(|r|r.circuit_node),"base":proposal.base,"state_digest":proposal.state_digest,"edits_digest":proposal.edits_digest,"affected_nodes":proposal.affected,"edits":edits,"summary":summary,
                "scope":proposal.before.id,
                "before":proposal.before.nodes.iter().filter(|n|proposal.affected.contains(&n.id)).collect::<Vec<_>>(),
                "after":proposal.after.nodes.iter().filter(|n|proposal.affected.contains(&n.id)).collect::<Vec<_>>()
            }),
        )?),
        _ => None,
    };
    let allowed = if let Some(guard) = &mut oversight {
        guard.confirm(ctx).await?
    } else {
        super::await_approval(
        ctx,
        "replan_proposal",
        summary,
        serde_json::json!({
            "request_type":"replan_proposal", "tool":"ReplanBlueprint", "proposal_id":proposal.id,
            "run_id":proposal.run_id, "source":proposal.source, "base":proposal.base,
            "state_digest":proposal.state_digest, "edits_digest":proposal.edits_digest,
            "affected_nodes":proposal.affected, "edits":edits, "summary":summary,
        }),
    )
    .await?
    };
    if !allowed {
        if matches!(source, Source::Supervisor { .. }) {
            return Err(diagnostic::error(Category::ApprovalRejected, Stage::Approval));
        }
        return Err(rejected("replan denied by user"));
    }
    if let Err(error) = verify(ctx, &proposal) {
        if let Some(guard) = &mut oversight { guard.failed(&error, Stage::VersionCheck)?; }
        return Err(error);
    }
    let mut state = ctx.blueprint_apply.lock();
    if state.pending.is_some() || state.blocked {
        if let Some(guard) = &mut oversight {
            guard.failed(&diagnostic::error(Category::Application, Stage::Application), Stage::Application)?;
        }
        return Err(rejected("another proposal is pending or recovery is required"));
    }
    state.pending = Some(proposal);
    if let Some(guard) = &mut oversight {
        guard.staged();
    }
    Ok(format!("{changed}; approved, pending the next safe node boundary"))
}

/// This is the only writer that may replace a running SharedBlueprint.
pub(crate) fn commit_boundary(
    ctx: &ExecutionContext,
    checkpoint: &mut ExecutionCheckpoint,
    sink: &dyn CheckpointSink,
) -> DaemonResult<()> {
    let pending = {
        let mut state = ctx.blueprint_apply.lock();
        if state.blocked {
            return Err(DaemonError::Persistence(
                "blueprint application failed; recovery required".into(),
            ));
        }
        checkpoint.blueprint_version = state.version.clone();
        if checkpoint.status != RunStatus::Running {
            state.pending = None;
        }
        // The interpreter owns a synchronous oversight gate boundary. No node
        // effects are in flight there; the gate itself cannot be edited.
        let oversight_gate = checkpoint.in_flight.is_some_and(|id| {
            checkpoint
                .view
                .root
                .as_ref()
                .and_then(|bp| bp.node(id))
                .is_some_and(|node| node.kind == "OversightCheckpoint")
        });
        if checkpoint.in_flight.is_some() && !oversight_gate {
            None
        } else {
            state.pending.take()
        }
    };
    let Some(proposal) = pending else {
        return sink.write(checkpoint);
    };
    // The completed node's successor queue is already constructed. Only future
    // parameter changes are permitted; a circuit retry has already been queued.
    let verification = verify(ctx, &proposal).and_then(|()| {
        if matches!(proposal.source, Source::Supervisor { .. }) {
            crate::oversight::actions::authorize_application(ctx, proposal.id)
        } else {
            Ok(())
        }
    });
    if matches!(proposal.source, Source::Supervisor { .. })
        && (verification.is_err()
            || proposal.affected.iter().any(|id| checkpoint.executed.contains(id)))
    {
        let error = verification.err().unwrap_or_else(|| diagnostic::error(Category::VersionStale, Stage::VersionCheck));
        crate::oversight::actions::fail(
            db(ctx)?,
            ctx.run_id,
            proposal.id,
            &error,
            Stage::VersionCheck,
        )?;
        return sink.write(checkpoint);
    }
    verification?;
    if checkpoint.run_id != proposal.run_id
        || proposal.affected.iter().any(|id| checkpoint.executed.contains(id))
    {
        return Err(rejected("affected node completed before the safe apply boundary"));
    }
    let oversight_id = matches!(proposal.source, Source::Supervisor { .. }).then_some(proposal.id);
    let result = commit(ctx, checkpoint, sink, proposal);
    if let Err(error) = &result {
        ctx.blueprint_apply.lock().blocked = true;
        if let Some(id) = oversight_id {
            crate::oversight::actions::fail(db(ctx)?, ctx.run_id, id, error, Stage::Application)?;
        }
    }
    result
}

fn commit(
    ctx: &ExecutionContext,
    checkpoint: &mut ExecutionCheckpoint,
    sink: &dyn CheckpointSink,
    proposal: Proposal,
) -> DaemonResult<()> {
    let db = db(ctx)?;
    let versions =
        ctx.version_manager.as_ref().ok_or_else(|| rejected("Version Flow unavailable"))?;
    let mut intent = ApplyIntent {
        proposal,
        checkpoint: checkpoint.clone(),
        after_version: None,
        committed: false,
        abandoned: false,
        recovery_note: String::new(),
    };
    let key = format!("blueprint-apply:{}", intent.proposal.id);
    db.put_durable(cf::EXECUTION_STATE, key.as_bytes(), &encode(&intent)?)?;
    let p = &intent.proposal;
    let (after, operation_id) = blueprint_files::save_with_origin(
        db,
        versions,
        &p.after,
        &p.base.blueprint_uri,
        &p.file,
        Some(&p.base).into(),
        Some(crate::execution::file_journal::FileOrigin {
            run_id: ctx.run_id,
            node_id: ctx.current_node,
            attempt: ctx.file_attempt,
            wal_position: ctx.transaction_log.mark(),
        }),
    )?;
    *ctx.blueprint.as_ref().ok_or_else(|| rejected("shared blueprint unavailable"))?.write() =
        p.after.clone();
    if let Some(operation_id) = operation_id {
        ctx.transaction_log.record_blueprint_application(operation_id);
    }
    checkpoint.transaction_log = ctx.transaction_log.entries();
    checkpoint.blueprint_version = Some(after.clone());
    checkpoint.view.root = Some(p.after.clone());
    checkpoint.view.invalidate(
        &p.affected,
        &[],
        crate::execution::blackboard::ChangeKind::BlueprintChanged,
    );
    checkpoint.view.record_change(
        crate::execution::blackboard::ChangeKind::BlueprintApplied,
        Vec::new(),
        Some(after.clone()),
        Some(p.id.to_string()),
    );
    intent.after_version = Some(after.clone());
    intent.checkpoint = checkpoint.clone();
    db.put_durable(cf::EXECUTION_STATE, key.as_bytes(), &encode(&intent)?)?;
    sink.write(checkpoint)?;
    intent.committed = true;
    db.put_durable(cf::EXECUTION_STATE, key.as_bytes(), &encode(&intent)?)?;
    ctx.blueprint_apply.lock().version = Some(after.clone());
    if matches!(intent.proposal.source, Source::Supervisor { .. }) {
        crate::oversight::actions::transition(
            db,
            ctx.run_id,
            intent.proposal.id,
            crate::oversight::requests::State::Applied,
            vec![
                format!("version:{}", after.snapshot_id),
                format!("blueprint-apply:{}", intent.proposal.id),
            ],
            "Blueprint changes committed at a safe execution boundary",
        )?;
    }
    let detail = serde_json::json!({"proposal_id":intent.proposal.id,"run_id":ctx.run_id,
        "source":intent.proposal.source,"before":intent.proposal.base,"after":after,"summary":intent.proposal.summary});
    ctx.audit("replan.applied", detail.clone());
    if let Some(events) = &ctx.events {
        let _ = events.send(crate::execution::ExecutionEvent::Message {
            node_id: ctx.current_node,
            message: format!("replan.applied {detail}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;

pub(crate) fn has_pending(ctx: &ExecutionContext) -> bool {
    ctx.blueprint_apply.lock().pending.is_some()
}

pub(crate) fn pending_review(ctx: &ExecutionContext) -> Option<Uuid> {
    match ctx.blueprint_apply.lock().pending.as_ref().map(|p| p.source) {
        Some(Source::Supervisor {
            review_id,
        }) => Some(review_id),
        _ => None,
    }
}

pub fn reconcile(
    db: &Db,
    versions: &crate::storage::versioning::VersionManager,
) -> DaemonResult<Vec<String>> {
    use crate::execution::file_journal::{DbFileJournal, FileJournalStore, FilePhase};
    use crate::execution::transaction::TransactionEntry;
    let operations = DbFileJournal(db.clone()).operations()?;
    let mut conflicts = Vec::new();
    for (key, bytes) in db.scan(cf::EXECUTION_STATE)? {
        if !key.starts_with(INTENT_PREFIX) {
            continue;
        }
        let mut intent: ApplyIntent = serde_json::from_slice(&bytes)
            .map_err(|e| rejected(&format!("Invalid apply intent: {e}")))?;
        let original = encode(&intent)?;
        let id = intent.proposal.id;
        let run = intent.proposal.run_id;
        let acknowledged = match intent.proposal.source {
            Source::Supervisor {
                review_id,
            } => crate::oversight::scheduler::load(db, run)?.is_some_and(|s| {
                s.reviews.iter().any(|r| {
                    r.review_id == review_id
                        && r.proposals.iter().any(|p| {
                            p.proposal_id == id
                                && p.state == crate::oversight::requests::State::Applied
                        })
                })
            }),
            _ => true,
        };
        // Settled history is not revalidated against later file changes or
        // snapshot retention. Only an unfinished acknowledgement needs repair.
        if intent.committed && acknowledged {
            continue;
        }
        let latest = DbCheckpointSink::load(db, run)?;
        let proof = if let Some(after) = &intent.after_version {
            let snapshot = versions.file_at_snapshot(after.snapshot_id, &after.blueprint_uri)?;
            let image_matches = snapshot.is_some_and(|(_, text)| {
                crate::storage::versioning::hash_content(text.as_bytes()) == after.blob_hash
            }) && crate::storage::versioning::hash_content(
                &intent.proposal.file,
            ) == after.blob_hash;
            let recorded = |cp: &ExecutionCheckpoint| {
                cp.run_id == run
                    && cp.view.changes.iter().any(|change| {
                        matches!(
                            change.kind,
                            crate::execution::blackboard::ChangeKind::BlueprintApplied
                        ) && change.evidence.as_deref() == Some(id.to_string().as_str())
                            && change.version.as_ref() == Some(after)
                    })
            };
            image_matches
                && intent.checkpoint.run_id == run
                && intent.checkpoint.blueprint_version.as_ref() == Some(after)
                && (intent.committed
                    || (intent.checkpoint.view.root.as_ref() == Some(&intent.proposal.after)
                        && recorded(&intent.checkpoint)
                        && latest.as_ref().is_some_and(recorded)))
        } else {
            false
        };
        if proof {
            intent.committed = true;
            intent.recovery_note="Committed version and checkpoint verified; acknowledgement reconciled without replay".into();
            if original != encode(&intent)? {
                db.put_durable(cf::EXECUTION_STATE, &key, &encode(&intent)?)?;
            }
            if matches!(intent.proposal.source, Source::Supervisor { .. }) {
                let after = intent.after_version.as_ref().expect("verified version");
                crate::oversight::actions::reconcile_applied(
                    db,
                    run,
                    id,
                    vec![format!("version:{}", after.snapshot_id), format!("blueprint-apply:{id}")],
                )?;
            }
            continue;
        }
        if intent.committed {
            return Err(rejected(&format!(
                "Committed apply {id} has inconsistent recovery evidence"
            )));
        }
        if !intent.abandoned {
            let recorded_operations: Vec<_> = latest
                .iter()
                .flat_map(|cp| &cp.transaction_log)
                .filter_map(|entry| match entry {
                    TransactionEntry::FileOperation {
                        operation_id,
                        ..
                    }
                    | TransactionEntry::BlueprintApplication {
                        operation_id,
                    } => Some(*operation_id),
                    _ => None,
                })
                .collect();
            let no_unknown_operations = operations.iter().all(|op| {
                op.origin.run_id != run
                    || recorded_operations.contains(&op.operation_id)
                    || op.phase == FilePhase::Reverted
            });
            let unchanged_checkpoint = latest.as_ref().map(digest).transpose()?.as_deref()
                == Some(&intent.proposal.state_digest);
            let unchanged_file = versions.verify_blueprint(&intent.proposal.base).is_ok()
                && blueprint_files::binding(db, intent.proposal.before.id)?.as_ref()
                    == Some(&intent.proposal.base);
            intent.abandoned = intent.after_version.is_none()
                && unchanged_checkpoint
                && unchanged_file
                && no_unknown_operations;
        }
        intent.recovery_note = if intent.abandoned {
            "Recovery found the base file, version and checkpoint; no committed application was recorded. A new proposal and decision are required.".into()
        } else {
            "File, version and checkpoint do not prove a committed application. Manual recovery is required; no write or approval was replayed.".into()
        };
        if !intent.abandoned {
            conflicts.push(format!("Apply {id}: {}", intent.recovery_note));
        }
        if original != encode(&intent)? {
            db.put_durable(cf::EXECUTION_STATE, &key, &encode(&intent)?)?;
        }
        if matches!(intent.proposal.source, Source::Supervisor { .. }) {
            crate::oversight::actions::reconcile_unfinished(db, run, id, &intent.recovery_note)?;
        }
    }
    Ok(conflicts)
}
