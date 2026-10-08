//! Crash/resume coverage at real interpreter commit boundaries.

use crate::common::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Simulate process loss immediately before or after a durable save. Panicking
/// unwinds the run instead of returning a storage error (covered separately by
/// persistence-failure tests). No production fault-injection hook is needed.
struct CrashSink {
    records: MemorySink,
    writes: AtomicUsize,
    cut: usize,
    after: bool,
}

impl CheckpointSink for CrashSink {
    fn run_id(&self) -> Uuid {
        self.records.run_id()
    }

    fn write(&self, checkpoint: &ExecutionCheckpoint) -> DaemonResult<()> {
        let crash = self.writes.fetch_add(1, Ordering::SeqCst) == self.cut;
        assert!(!crash || self.after, "injected crash before save");
        self.records.write(checkpoint)?;
        assert!(!crash, "injected crash after save");
        Ok(())
    }
}

fn interpreter(registry: Arc<Registry>, sink: Arc<dyn CheckpointSink>) -> Interpreter {
    Interpreter::new(registry, LlmClientFactory::new(), std::env::temp_dir())
        .with_checkpoint_sink(sink)
}

/// Compare the entire durable state, ignoring clocks, run ids and set ordering.
/// Tree ids are deterministic: comparing the tree also detects skipped or
/// duplicated iterations even when their final variable value is identical.
fn normalized(checkpoint: &ExecutionCheckpoint) -> serde_json::Value {
    let mut value = serde_json::to_value(checkpoint).unwrap();
    let obj = value.as_object_mut().unwrap();
    for key in ["run_id", "started_at", "updated_at"] {
        obj.remove(key);
    }
    for key in ["executed", "triggered"] {
        obj[key].as_array_mut().unwrap().sort_by_key(|v| v.to_string());
    }
    for node in obj["exec_tree"]["nodes"].as_object_mut().unwrap().values_mut() {
        node.as_object_mut().unwrap().remove("started_at_ms");
        node.as_object_mut().unwrap().remove("finished_at_ms");
    }
    for invocation in obj["view"]["invocations"].as_array_mut().unwrap() {
        invocation.as_object_mut().unwrap().remove("started_at");
        invocation.as_object_mut().unwrap().remove("finished_at");
    }
    value
}

async fn assert_every_save_recovers(
    blueprint: Blueprint,
    registry: Arc<Registry>,
) -> Vec<ExecutionCheckpoint> {
    let sink = Arc::new(MemorySink::new());
    interpreter(registry.clone(), sink.clone())
        .run(&shared(blueprint.clone()), None)
        .await
        .unwrap();
    let checkpoints = sink.checkpoints.lock().unwrap().clone();
    let expected = checkpoints.last().unwrap();
    assert_eq!(expected.status, RunStatus::Completed);
    for cut in 0..checkpoints.len() {
        for after in [false, true] {
            let sink = Arc::new(CrashSink {
                records: MemorySink::new(),
                writes: AtomicUsize::new(0),
                cut,
                after,
            });
            let mut runner = interpreter(registry.clone(), sink.clone());
            let bp = shared(blueprint.clone());
            let crash = tokio::spawn(async move { runner.run(&bp, None).await })
                .await
                .expect_err("the chosen save must be reached");
            assert!(crash.is_panic());
            let saved = sink.records.checkpoints.lock().unwrap().last().cloned();
            let Some(saved) = saved else {
                assert_eq!((cut, after), (0, false));
                continue; // No durable run exists before the seed is saved.
            };
            // Exercise the storage representation, not just a clone in memory.
            let saved: ExecutionCheckpoint =
                serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
            let resumed_sink = Arc::new(MemorySink::new());
            let result = interpreter(registry.clone(), resumed_sink.clone())
                .resume_with_control(
                    &shared(blueprint.clone()),
                    saved.clone(),
                    None,
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                )
                .await;
            if saved.in_flight.is_some() {
                assert!(result.unwrap_err().to_string().contains("manual recovery required"));
                assert!(resumed_sink.checkpoints.lock().unwrap().is_empty());
            } else if saved.status.is_terminal() {
                assert_eq!(normalized(&saved), normalized(expected));
                assert!(result.is_err(), "terminal runs must not be replayed");
                assert!(resumed_sink.checkpoints.lock().unwrap().is_empty());
            } else {
                result.unwrap();
                let records = resumed_sink.checkpoints.lock().unwrap();
                assert_eq!(
                    normalized(records.last().unwrap()),
                    normalized(expected),
                    "state diverged at save {cut}, after={after}",
                );
            }
        }
    }
    checkpoints
}

fn append_end(blueprint: &mut Blueprint, predecessor: Uuid) -> Uuid {
    let output = blueprint
        .node(predecessor)
        .unwrap()
        .pins
        .iter()
        .find(|p| p.pin_type == PinType::ExecOutput)
        .unwrap()
        .id;
    let id = Uuid::new_v4();
    let input = Uuid::new_v4();
    blueprint.nodes.push(Node {
        id,
        kind: "End".into(),
        node_type: NodeType::Event,
        position: (0.0, 0.0),
        pins: vec![Pin::exec(PinType::ExecInput, input)],
        data: serde_json::Value::Null,
    });
    blueprint.edges.push(Edge {
        id: Uuid::new_v4(),
        source_node: predecessor,
        source_pin: output,
        target_node: id,
        target_pin: input,
    });
    id
}

#[tokio::test]
async fn ordinary_transitions_recover_before_and_after_every_save() {
    assert_every_save_recovers(build_blueprint(), Arc::new(Registry::with_builtins())).await;
}

#[tokio::test]
async fn nested_function_entry_exit_and_return_recover_at_every_save() {
    let inner = add_function();
    let mut outer = add_function();
    outer.name = "Outer".into();
    let nested = outer.body.nodes.iter_mut().find(|n| n.kind == "Add").unwrap();
    nested.kind = "CallFunction".into();
    nested.data = serde_json::json!({"function": "AddFunc"});
    let registry = Arc::new(Registry::with_builtins());
    registry.register_function(inner);
    registry.register_function(outer);
    let mut bp = call_function_blueprint("Outer");
    let caller = bp.nodes.iter().find(|n| n.kind == "CallFunction").unwrap().id;
    let end = append_end(&mut bp, caller);
    let checkpoints = assert_every_save_recovers(bp, registry).await;
    assert!(checkpoints.iter().any(|cp| cp.call_stack.len() == 3));
    assert!(checkpoints.iter().any(|cp| cp.call_stack.len() == 1
        && cp.executed.contains(&caller)
        && cp.pending.contains(&end)));
    for cp in checkpoints {
        assert_eq!(cp.variables.len(), cp.call_stack.len());
        assert_eq!(cp.frame_trees.len() + 1, cp.call_stack.len());
    }
}

#[tokio::test]
async fn foreach_entry_advance_and_completion_recover_at_every_save() {
    let bp = crate::foreach::foreach_blueprint(serde_json::json!([1, 2, 3]));
    let checkpoints = assert_every_save_recovers(bp, Arc::new(Registry::with_builtins())).await;
    for index in 0..3 {
        assert!(checkpoints.iter().any(|cp| cp.foreach_stack.last().is_some_and(
            |state| state.index == index
                && state.count == index as u32 + 1
                && !cp.pending.is_empty()
        )));
    }
    assert_eq!(checkpoints.last().unwrap().variables[0]["last"], Value::Int(3));
}

#[tokio::test]
async fn empty_foreach_dispatches_completed_before_save() {
    let mut bp = crate::foreach::foreach_blueprint(serde_json::json!([]));
    // Only the empty loop's continuation executes. Keep the body intact so
    // accidentally taking Body would be visible in the durable execution tree.
    let get = bp.nodes.iter_mut().find(|n| n.kind == "VariableGet").unwrap();
    get.kind = "Start".into();
    get.data = serde_json::json!({"Value": 3});
    let checkpoints = assert_every_save_recovers(bp, Arc::new(Registry::with_builtins())).await;
    assert!(checkpoints.iter().all(|cp| cp.foreach_stack.is_empty()));
}

#[tokio::test]
async fn foreach_inside_function_restores_loop_depth_and_locals() {
    use metteur_shared::model::function::{FunctionSignature, FunctionSource};
    let mut body = crate::foreach::foreach_blueprint(serde_json::json!([1, 2, 3]));
    body.nodes.iter_mut().find(|n| n.kind == "Start").unwrap().kind = "FunctionEntry".into();
    body.nodes.iter_mut().find(|n| n.kind == "End").unwrap().kind = "FunctionExit".into();
    let registry = Arc::new(Registry::with_builtins());
    registry.register_function(FunctionEntry {
        id: body.id,
        name: "Loop".into(),
        description: String::new(),
        signature: FunctionSignature {
            inputs: vec![],
            outputs: vec![],
        },
        body,
        source: FunctionSource::Builtin,
    });
    let mut bp = call_function_blueprint("Loop");
    let caller = bp.nodes.iter().find(|n| n.kind == "CallFunction").unwrap().id;
    append_end(&mut bp, caller);
    let checkpoints = assert_every_save_recovers(bp, registry).await;
    assert!(checkpoints.iter().any(|cp| cp.foreach_stack.last().is_some_and(|s| s.depth == 2)));
    assert!(checkpoints.last().unwrap().variables[0].is_empty());
}

pub(crate) fn failing_validator_blueprint(attempts: u32) -> Blueprint {
    let mut bp = build_blueprint();
    let validator = bp.nodes.iter_mut().find(|n| n.kind == "Judge").unwrap();
    validator.kind = "Validator".into();
    validator.pins.iter_mut().find(|p| p.name == "Score").unwrap().name = "Actual".into();
    validator.pins.iter_mut().find(|p| p.name == "Success").unwrap().name = "Passed".into();
    validator.pins.push(pin_with_default(
        Uuid::new_v4(),
        "Expected",
        PinType::DataInput,
        DataType::Float,
        serde_json::json!(99),
    ));
    validator.pins.push(Pin::exec(PinType::ExecOutput, Uuid::new_v4()));
    validator.data = serde_json::json!({"mode": "eq", "retry": {"max_attempts": attempts}});
    let id = validator.id;
    append_end(&mut bp, id);
    bp
}

#[tokio::test]
async fn retry_queue_attempts_and_marks_recover_at_every_save() {
    let bp = failing_validator_blueprint(3);
    let validator = bp.nodes.iter().find(|n| n.kind == "Validator").unwrap().id;
    let end = bp.nodes.iter().find(|n| n.kind == "End").unwrap().id;
    let checkpoints = assert_every_save_recovers(bp, Arc::new(Registry::with_builtins())).await;
    for attempt in [1, 2] {
        let retry = checkpoints
            .iter()
            .find(|cp| {
                cp.attempt_counts.get(&validator) == Some(&attempt)
                    && !cp.executed.contains(&validator)
            })
            .expect("retry boundary must be persisted");
        assert!(retry.pending.contains(&validator));
        assert!(!retry.pending.contains(&end));
        assert!(retry.executed_order.is_empty());
        assert!(retry.validation_marks.contains_key(&validator));
        assert_eq!(retry.circuit_failures, 0);
    }
}

#[tokio::test]
async fn legacy_and_future_records_are_readable_but_not_guessed_complete() {
    let bp = build_blueprint();
    let sink = Arc::new(MemorySink::new());
    new_interpreter()
        .with_checkpoint_sink(sink.clone())
        .run(&shared(bp.clone()), None)
        .await
        .unwrap();
    let checkpoint = sink.checkpoints.lock().unwrap()[1].clone();
    for version in [None, Some(999)] {
        for has_pending in [false, true] {
            let mut json = serde_json::to_value(&checkpoint).unwrap();
            json.as_object_mut().unwrap().remove("transition_version");
            if let Some(version) = version {
                json["transition_version"] = version.into();
            }
            if !has_pending {
                json["pending"] = serde_json::json!([]);
            }
            let legacy = serde_json::from_value(json).unwrap();
            let output = Arc::new(MemorySink::new());
            let error = new_interpreter()
                .with_checkpoint_sink(output.clone())
                .resume_with_control(
                    &shared(bp.clone()),
                    legacy,
                    None,
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("manual recovery required"));
            assert!(output.checkpoints.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn resume_rejects_a_different_blueprint_without_writing() {
    let bp = build_blueprint();
    let checkpoint = checkpoint_after_start(&bp);
    let output = Arc::new(MemorySink::new());
    let error = new_interpreter()
        .with_checkpoint_sink(output.clone())
        .resume_with_control(
            &shared(build_blueprint()),
            checkpoint,
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("different blueprint"));
    assert!(output.checkpoints.lock().unwrap().is_empty());
}

#[tokio::test]
async fn resumed_pause_remains_cancellable_and_records_cancelled() {
    let bp = build_blueprint();
    let checkpoint = checkpoint_after_start(&bp);
    let sink = Arc::new(MemorySink::new());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut runner = new_interpreter().with_checkpoint_sink(sink.clone());
    let flag = cancel.clone();
    let task = tokio::spawn(async move {
        runner
            .resume_with_control(
                &shared(bp),
                checkpoint,
                None,
                Arc::new(AtomicBool::new(true)),
                flag,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while sink.checkpoints.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        cancel.store(true, Ordering::SeqCst);
        assert!(matches!(task.await.unwrap(), Err(DaemonError::Interrupted(_))));
    })
    .await
    .unwrap();
    let records = sink.checkpoints.lock().unwrap();
    assert_eq!(records.last().unwrap().status, RunStatus::Cancelled);
    assert_eq!(records.last().unwrap().executed, records[0].executed);
}
