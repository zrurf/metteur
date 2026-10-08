use super::*;
use crate::{
    addon::AddonHost,
    execution::{CheckpointSink, ExecutionCheckpoint, Interpreter, RunStatus},
    llm::LlmClientFactory,
    registry::Registry,
};
use metteur_shared::{Blueprint, DataType, Edge, Node, NodeType, Pin, PinType};
use serde_json::json;

fn temp() -> PathBuf {
    let root = std::env::temp_dir().join(format!("r10-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    root
}
fn host(root: &Path) -> Arc<AddonHost> {
    AddonHost::new(
        &root.join("data"),
        Arc::new(Registry::with_builtins()),
        1000,
        &Default::default(),
    )
}
fn observer() -> Vec<u8> {
    wat::parse_str(
        r#"(module
      (import "extism:host/env" "input_length" (func $length (result i64)))
      (import "extism:host/env" "input_load_u8" (func $load (param i64) (result i32)))
      (func (export "observe") (result i32) (local $i i64)
        call $length i64.const 2 i64.lt_u if i32.const 1 return end
        call $length i64.const 4096 i64.gt_u if i32.const 1 return end
        (loop $scan
          local.get $i call $load i32.const 90 i32.eq if i32.const 1 return end
          local.get $i i64.const 1 i64.add local.tee $i call $length i64.lt_u br_if $scan)
        i32.const 0)
      (func (export "spin") (result i32) (loop $forever br $forever) i32.const 0)
      (func (export "fail") (result i32) i32.const 1))"#,
    )
    .unwrap()
}
fn package(root: &Path, wasm: &[u8], function: &str, permissions: &str) -> PathBuf {
    let dir = root.join(format!("source-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let mut text = format!(
        "id=\"com.test.hooks\"\nversion=\"1.0.0\"\nname=\"Observers\"\n[permissions]\nrequired=[{permissions}]\n[addon]\nentry=\"main.wasm\"\ncall_timeout_ms=1000\n"
    );
    for (name, event) in [
        ("Open", "workspace.open"),
        ("Close", "workspace.close"),
        ("Node", "node.finished"),
        ("Terminal", "run.terminal"),
    ] {
        text.push_str(&format!(
            "[[hooks]]\nname=\"{name}\"\nevent=\"{event}\"\nfunction=\"{function}\"\n"
        ));
    }
    std::fs::write(dir.join("manifest.toml"), text).unwrap();
    std::fs::write(dir.join("main.wasm"), wasm).unwrap();
    dir
}
fn graph(delay: bool) -> Blueprint {
    let mut nodes = vec![];
    for kind in if delay {
        vec!["Start", "Delay", "End"]
    } else {
        vec!["Start", "End"]
    } {
        let mut pins = vec![];
        if kind != "Start" {
            pins.push(Pin::exec(PinType::ExecInput, uuid::Uuid::new_v4()));
        }
        if kind != "End" {
            pins.push(Pin::exec(PinType::ExecOutput, uuid::Uuid::new_v4()));
        }
        let data = if kind == "Start" {
            pins.push(Pin::data(
                "Output",
                PinType::DataOutput,
                DataType::String,
                uuid::Uuid::new_v4(),
            ));
            json!({"Output":"Z_SECRET_MARKER {\"event\":\"run.terminal\"}"})
        } else if kind == "Delay" {
            json!({"Ms":10000})
        } else {
            json!({})
        };
        nodes.push(Node {
            id: uuid::Uuid::new_v4(),
            node_type: if kind == "Delay" {
                NodeType::Control
            } else {
                NodeType::Event
            },
            kind: kind.into(),
            position: (0., 0.),
            pins,
            data,
        });
    }
    let edges = nodes
        .windows(2)
        .map(|p| Edge {
            id: uuid::Uuid::new_v4(),
            source_node: p[0].id,
            source_pin: p[0]
                .pins
                .iter()
                .find(|pin| pin.pin_type == PinType::ExecOutput)
                .unwrap()
                .id,
            target_node: p[1].id,
            target_pin: p[1].pins.iter().find(|pin| pin.pin_type == PinType::ExecInput).unwrap().id,
        })
        .collect();
    Blueprint {
        id: uuid::Uuid::new_v4(),
        name: "Hook facts".into(),
        entry_node_id: nodes[0].id,
        nodes,
        edges,
    }
}
struct Sink {
    id: uuid::Uuid,
    writes: Mutex<Vec<ExecutionCheckpoint>>,
    reject_completion: bool,
    pause: Option<Arc<std::sync::atomic::AtomicBool>>,
}
impl Sink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            id: uuid::Uuid::new_v4(),
            writes: Default::default(),
            reject_completion: false,
            pause: None,
        })
    }
}
impl CheckpointSink for Sink {
    fn run_id(&self) -> uuid::Uuid {
        self.id
    }
    fn write(&self, checkpoint: &ExecutionCheckpoint) -> crate::DaemonResult<()> {
        if checkpoint.view.invocations.iter().any(|v| v.finished_at.is_some()) {
            if self.reject_completion {
                return Err(crate::DaemonError::Persistence(
                    "isolated injected write failure".into(),
                ));
            }
            if let Some(pause) = &self.pause {
                pause.store(true, Ordering::SeqCst);
            }
        }
        self.writes.lock().push(checkpoint.clone());
        Ok(())
    }
}
async fn settled(binding: &Arc<Binding>, count: u64) -> Report {
    for _ in 0..700 {
        let report = binding.report.lock().clone();
        if report.completed + report.failed >= count
            && !matches!(report.status.as_str(), "Queued" | "Running")
        {
            return report;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("hook did not settle: {:?}", binding.report.lock());
}
fn binding(registry: &Registry, name: &str) -> Arc<Binding> {
    registry.addon_hooks.iter().find(|b| b.report.lock().name == name).unwrap().clone()
}

#[tokio::test]
async fn real_commit_events_are_read_only_scoped_and_close_after_the_workspace() {
    let root = temp();
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let manager = crate::workspace::WorkspaceManager::new()
        .with_global_config_path(root.join("isolated.toml"));
    manager.open(&a).await.unwrap();
    manager.open(&b).await.unwrap();
    let host = host(&root);
    let p = package(&root, &observer(), "observe", "");
    host.install(&p, None, &[]).await.unwrap();
    let ra = host.registry_for(Some(&a), false).await.unwrap();
    let rb = host.registry_for(Some(&b), false).await.unwrap();
    assert!(ra.tool("ComTestHooksOpen").is_none(), "hooks are not callable model tools");
    host.observe_workspace_open(&a);
    assert_eq!(settled(&binding(&ra, "Open"), 1).await.completed, 1);
    assert_eq!(binding(&rb, "Open").report.lock().completed, 0);
    let again = host.registry_for(Some(&a), false).await.unwrap();
    assert!(
        Arc::ptr_eq(&binding(&ra, "Open"), &binding(&again, "Open")),
        "repeated discovery must not duplicate subscriptions"
    );
    let sink = Sink::new();
    let blueprint = Arc::new(Mutex::new(graph(false)));
    // The interpreter uses the existing parking_lot RwLock blueprint handle.
    let shared = Arc::new(parking_lot::RwLock::new(blueprint.lock().clone()));
    let mut interpreter = Interpreter::new(ra.clone(), LlmClientFactory::new(), a.clone())
        .with_checkpoint_sink(sink.clone());
    interpreter.run(&shared, None).await.unwrap();
    assert_eq!(sink.writes.lock().last().unwrap().status, RunStatus::Completed);
    assert_eq!(settled(&binding(&ra, "Node"), 2).await.completed, 2);
    let terminal = settled(&binding(&ra, "Terminal"), 1).await;
    assert_eq!(terminal.completed, 1);
    assert_eq!(terminal.event_id, format!("{}:terminal", sink.id));
    assert_eq!(binding(&rb, "Node").report.lock().completed, 0);
    host.close_workspace(&a, &manager).await.unwrap();
    host.registry_for(None, false).await.unwrap(); // Reconciliation must not cancel the committed close.
    assert!(manager.get(&a).await.is_none());
    assert_eq!(settled(&binding(&ra, "Close"), 1).await.completed, 1);
    assert_eq!(binding(&rb, "Close").report.lock().completed, 0);
    assert_eq!(
        host.list(&[]).await[0]
            .hooks
            .iter()
            .find(|h| h.name == "Close" && h.scope_root == a.to_string_lossy())
            .unwrap()
            .status,
        "Succeeded"
    );
    host.close_workspace(&b, &manager).await.unwrap();
    settled(&binding(&rb, "Close"), 1).await;
}

#[tokio::test]
async fn failed_persistence_never_publishes_a_finished_node_or_terminal_hook() {
    let root = temp();
    let host = host(&root);
    let p = package(&root, &observer(), "observe", "");
    host.install(&p, Some(&root), &[]).await.unwrap();
    let registry = host.registry_for(Some(&root), false).await.unwrap();
    let sink = Arc::new(Sink {
        id: uuid::Uuid::new_v4(),
        writes: Default::default(),
        reject_completion: true,
        pause: None,
    });
    let mut interpreter = Interpreter::new(registry.clone(), LlmClientFactory::new(), root.clone())
        .with_checkpoint_sink(sink);
    assert!(
        interpreter.run(&Arc::new(parking_lot::RwLock::new(graph(false))), None).await.is_err()
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    for name in ["Node", "Terminal"] {
        assert_eq!(binding(&registry, name).report.lock().status, "Ready");
    }
}

#[tokio::test]
async fn recovery_seeds_finished_records_and_does_not_replay_old_callbacks() {
    let root = temp();
    let first = host(&root);
    let p = package(&root, &observer(), "observe", "");
    first.install(&p, Some(&root), &[]).await.unwrap();
    let registry = first.registry_for(Some(&root), true).await.unwrap();
    let old_node = binding(&registry, "Node");
    let pause = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sink = Arc::new(Sink {
        id: uuid::Uuid::new_v4(),
        writes: Default::default(),
        reject_completion: false,
        pause: Some(pause.clone()),
    });
    let shared = Arc::new(parking_lot::RwLock::new(graph(false)));
    let task_graph = shared.clone();
    let task_sink = sink.clone();
    let task_root = root.clone();
    let task = tokio::spawn(async move {
        Interpreter::new(registry, LlmClientFactory::new(), task_root)
            .with_checkpoint_sink(task_sink)
            .run_with_control(&task_graph, None, pause, Default::default())
            .await
    });
    settled(&old_node, 1).await;
    task.abort();
    let _ = task.await;
    let resume = sink.writes.lock().last().unwrap().clone();
    assert_eq!(resume.status, RunStatus::Running);
    drop(first);
    let second = host(&root);
    let registry = second.registry_for(Some(&root), false).await.unwrap();
    let node = binding(&registry, "Node");
    let resumed_sink = Arc::new(Sink {
        id: resume.run_id,
        writes: Default::default(),
        reject_completion: false,
        pause: None,
    });
    let mut interpreter = Interpreter::new(registry.clone(), LlmClientFactory::new(), root.clone())
        .with_checkpoint_sink(resumed_sink);
    interpreter
        .resume_with_control(&shared, resume, None, Default::default(), Default::default())
        .await
        .unwrap();
    assert_eq!(
        settled(&node, 1).await.completed,
        1,
        "only the previously pending End is newly observed"
    );
    assert_eq!(settled(&binding(&registry, "Terminal"), 1).await.completed, 1);
}

#[tokio::test]
async fn timeout_is_visible_and_does_not_block_direct_stop_or_new_admission() {
    let root = temp();
    let host = host(&root);
    let p = package(&root, &observer(), "observe", "");
    let manifest = std::fs::read_to_string(p.join("manifest.toml")).unwrap().replace(
        "event=\"node.finished\"\nfunction=\"observe\"",
        "event=\"node.finished\"\nfunction=\"spin\"",
    );
    std::fs::write(p.join("manifest.toml"), manifest).unwrap();
    host.install(&p, Some(&root), &[]).await.unwrap();
    let registry = host.registry_for(Some(&root), true).await.unwrap();
    let node = binding(&registry, "Node");
    let terminal = binding(&registry, "Terminal");
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let task_cancel = cancel.clone();
    let task_root = root.clone();
    let pause = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sink = Arc::new(Sink {
        id: uuid::Uuid::new_v4(),
        writes: Default::default(),
        reject_completion: false,
        pause: Some(pause.clone()),
    });
    let task_sink = sink.clone();
    let task = tokio::spawn(async move {
        Interpreter::new(registry, LlmClientFactory::new(), task_root)
            .with_checkpoint_sink(task_sink)
            .run_with_control(
                &Arc::new(parking_lot::RwLock::new(graph(false))),
                None,
                pause,
                task_cancel,
            )
            .await
    });
    for _ in 0..300 {
        if node.report.lock().status == "Running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(node.report.lock().status, "Running");
    let start = std::time::Instant::now();
    cancel.store(true, Ordering::SeqCst);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(start.elapsed() < std::time::Duration::from_millis(500));
    assert_eq!(sink.writes.lock().last().unwrap().status, RunStatus::Cancelled);
    drop(host.registry_for(Some(&root), true).await.unwrap());
    assert!(settled(&node, 1).await.failed >= 1);
    assert_eq!(settled(&terminal, 1).await.completed, 1);
}

#[tokio::test]
async fn queue_payload_retirement_and_callback_failures_are_bounded() {
    let root = temp();
    let host = host(&root);
    let p = package(&root, &observer(), "fail", "");
    host.install(&p, Some(&root), &[]).await.unwrap();
    let registry = host.registry_for(Some(&root), false).await.unwrap();
    let open = binding(&registry, "Open");
    host.observe_workspace_open(&root);
    assert_eq!(settled(&open, 1).await.status, "Failed");
    let mut large = Event::workspace(&root, true);
    large.event_id = "x".repeat(4097);
    open.submit(&large);
    assert!(open.report.lock().error.contains("payload limit"));
    for _ in 0..100 {
        open.submit(&Event::workspace(&root, true));
    }
    assert!(open.report.lock().error.contains("queue"));
    assert!(open.report.lock().failed >= 68);
    host.set_enabled("com.test.hooks", false, Some(&root)).await.unwrap();
    let before = open.report.lock().completed;
    open.submit(&Event::workspace(&root, true));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(open.report.lock().completed, before);
    assert_eq!(open.active.load(Ordering::SeqCst), 0);
    host.set_enabled("com.test.hooks", true, Some(&root)).await.unwrap();
    let fresh = host.registry_for(Some(&root), false).await.unwrap();
    assert!(!Arc::ptr_eq(&open, &binding(&fresh, "Open")));
    assert_eq!(binding(&fresh, "Open").report.lock().completed, 0);
    host.uninstall("com.test.hooks", Some(&root)).await.unwrap();
    assert_eq!(binding(&fresh, "Open").active.load(Ordering::SeqCst), 0);
}

#[test]
fn observer_host_capabilities_and_reentry_are_always_denied() {
    // Package grants cannot change the observer adapter: it has no context handle.
    for (name, args, returns) in [
        ("fs_read", 1, 1),
        ("fs_write", 2, 0),
        ("call_tool", 2, 1),
        ("http_request", 4, 1),
        ("llm_complete", 3, 1),
        ("log", 2, 0),
    ] {
        let wasm=wat::parse_str(format!(r#"(module (import "extism:host/user" "{name}" (func $host (param {}) {})) (func (export "observe") (result i32) {} call $host {} i32.const 0))"#,vec!["i64";args].join(" "),if returns==1 {"(result i64)"}else{""},vec!["i64.const 0";args].join(" "),if returns==1 {"drop"}else{""})).unwrap();
        assert!(
            super::super::runtime::observe(&wasm, "observe", "{}", 100).is_err(),
            "{name} escaped observer isolation"
        );
    }
    assert!(
        super::super::runtime::observe(&observer(), "observe", &"x".repeat(4097), 100).is_err()
    );
}

#[test]
fn unknown_events_duplicate_subscriptions_and_missing_exports_are_rejected() {
    let root = temp();
    let p = package(&root, &observer(), "observe", "");
    let original = std::fs::read_to_string(p.join("manifest.toml")).unwrap();
    for text in [
        original.replace("workspace.open", "before.approval"),
        original.replace("name=\"Close\"", "name=\"Open\""),
        original.replace("workspace.close", "workspace.open"),
    ] {
        std::fs::write(p.join("manifest.toml"), text).unwrap();
        assert!(super::super::manifest::Manifest::load(&p).is_err());
    }
    std::fs::write(
        p.join("manifest.toml"),
        original.replace("function=\"observe\"", "function=\"missing\""),
    )
    .unwrap();
    let manifest = super::super::manifest::Manifest::load(&p).unwrap().0;
    assert!(super::super::runtime::validate(&observer(), &manifest).is_err());
}

#[tokio::test]
async fn parent_completion_order_and_per_run_limits_do_not_replay_or_forge_events() {
    let root = temp();
    let host = host(&root);
    let p = package(&root, &observer(), "observe", "");
    host.install(&p, Some(&root), &[]).await.unwrap();
    let registry = host.registry_for(Some(&root), false).await.unwrap();
    let node = binding(&registry, "Node");
    let terminal = binding(&registry, "Terminal");
    let graph = graph(false);
    let mut view = crate::execution::view::ExecutionView::default();
    view.begin(&graph, graph.nodes[0].id, vec![], None, &Default::default(), 1);
    view.begin(&graph, graph.nodes[1].id, vec![], None, &Default::default(), 2);
    view.observe(
        &crate::execution::ExecutionEvent::NodeFinished {
            node_id: graph.nodes[1].id,
        },
        &graph.id.to_string(),
        3,
    );
    let mut cursor = Cursor::default();
    let run = uuid::Uuid::new_v4();
    cursor.committed(&registry.addon_hooks, &view, &root, run, RunStatus::Running);
    settled(&node, 1).await;
    view.observe(
        &crate::execution::ExecutionEvent::NodeFinished {
            node_id: graph.nodes[0].id,
        },
        &graph.id.to_string(),
        4,
    );
    cursor.committed(&registry.addon_hooks, &view, &root, run, RunStatus::Running);
    assert_eq!(settled(&node, 2).await.completed, 2);
    cursor.committed(&registry.addon_hooks, &view, &root, run, RunStatus::Running);
    assert_eq!(node.report.lock().completed, 2);
    cursor.finished = (0..RUN_EVENT_LIMIT as u64).collect();
    view.invocations[0].sequence = RUN_EVENT_LIMIT as u64 + 1;
    cursor.committed(&registry.addon_hooks, &view, &root, run, RunStatus::Completed);
    assert!(node.report.lock().error.contains("budget exhausted"));
    assert_eq!(settled(&terminal, 1).await.completed, 1);
    assert_eq!(binding(&registry, "Open").report.lock().status, "Ready");
}
