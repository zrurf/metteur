use super::*;
use crate::{
    addon::{AddonHost, nodes::tests as node_fixture},
    execution::{ExecutionEvent, Interpreter},
    llm::LlmClientFactory,
};
use metteur_shared::{Blueprint, DataType, Edge, Node, Pin};
use std::{path::Path, sync::Arc};

fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("r12-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&p).unwrap();
    p
}
fn host(base: &Path) -> Arc<AddonHost> {
    AddonHost::new(
        &base.join("data"),
        Arc::new(Registry::with_builtins()),
        1000,
        &metteur_shared::config::AddonConfig {
            require_signature: true,
            ..Default::default()
        },
    )
}
fn sign(path: &Path) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[29; 32]);
    std::fs::write(
        path.join("signature.toml"),
        crate::addon::signature::sign_package(path, &key).unwrap(),
    )
    .unwrap();
}
fn body(kind: &str) -> Blueprint {
    let signature = if kind == "Add" {
        metteur_shared::node_catalog::builtin_signature("Add").unwrap()
    } else {
        let mut s = node_fixture::signature();
        s.kind = kind.into();
        s.executor_kind = kind.into();
        s
    };
    let entry = Node {
        id: uuid::Uuid::new_v4(),
        kind: "FunctionEntry".into(),
        node_type: NodeType::Event,
        position: (0., 0.),
        pins: vec![
            Pin::exec(PinType::ExecOutput, uuid::Uuid::new_v4()),
            Pin::data("A", PinType::DataOutput, DataType::Int, uuid::Uuid::new_v4()),
        ],
        data: serde_json::json!({}),
    };
    let compute = Node {
        id: uuid::Uuid::new_v4(),
        kind: kind.into(),
        node_type: signature.node_type,
        position: (100., 0.),
        pins: signature.pins.iter().map(|p| p.instantiate(uuid::Uuid::new_v4())).collect(),
        data: serde_json::json!({"b":7}),
    };
    let exit = Node {
        id: uuid::Uuid::new_v4(),
        kind: "FunctionExit".into(),
        node_type: NodeType::Event,
        position: (200., 0.),
        pins: vec![
            Pin::exec(PinType::ExecInput, uuid::Uuid::new_v4()),
            Pin::data(
                "Result",
                PinType::DataInput,
                if kind == "Add" {
                    DataType::Float
                } else {
                    DataType::Int
                },
                uuid::Uuid::new_v4(),
            ),
        ],
        data: serde_json::json!({}),
    };
    let link = |a: &Node, ap: PinType, an: &str, b: &Node, bp: PinType, bn: &str| Edge {
        id: uuid::Uuid::new_v4(),
        source_node: a.id,
        source_pin: a
            .pins
            .iter()
            .find(|p| p.pin_type == ap && (an.is_empty() || p.name == an))
            .unwrap()
            .id,
        target_node: b.id,
        target_pin: b
            .pins
            .iter()
            .find(|p| p.pin_type == bp && (bn.is_empty() || p.name == bn))
            .unwrap()
            .id,
    };
    let edges = vec![
        link(&entry, PinType::ExecOutput, "", &compute, PinType::ExecInput, ""),
        link(&compute, PinType::ExecOutput, "", &exit, PinType::ExecInput, ""),
        link(&entry, PinType::DataOutput, "A", &compute, PinType::DataInput, "A"),
        link(&compute, PinType::DataOutput, "Result", &exit, PinType::DataInput, "Result"),
    ];
    Blueprint {
        id: uuid::Uuid::new_v4(),
        name: "Package function".into(),
        entry_node_id: entry.id,
        nodes: vec![entry, compute, exit],
        edges,
    }
}
fn package(
    base: &Path,
    id: &str,
    body: &Blueprint,
    dependencies: &[&str],
    with_node: bool,
) -> PathBuf {
    let p = if with_node {
        node_fixture::package(
            base,
            "1.0.0",
            &node_fixture::signature(),
            &node_fixture::wasm(r#"{"outputs":{"result":14}}"#),
        )
    } else {
        let p = base.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&p).unwrap();
        std::fs::write(
            p.join("manifest.toml"),
            format!("id=\"{id}\"\nversion=\"1.0.0\"\nname=\"Library\"\n"),
        )
        .unwrap();
        p
    };
    let mut text = std::fs::read_to_string(p.join("manifest.toml")).unwrap();
    text.push_str(&format!(
        "\n[[functions]]\nname=\"Library\"\nfile=\"library.blueprint\"\ndependencies=[{}]\n",
        dependencies.iter().map(|d| format!("\"{d}\"")).collect::<Vec<_>>().join(",")
    ));
    std::fs::write(p.join("manifest.toml"), text).unwrap();
    std::fs::write(
        p.join("library.blueprint"),
        crate::storage::blueprint_files::encode_native(body).unwrap(),
    )
    .unwrap();
    sign(&p);
    p
}
fn caller(reg: &Registry, name: &str) -> Blueprint {
    metteur_shared::dsl::compile_with_catalog(&format!("entry s: Start\nf: CallFunction(function = \"{name}\", A = 7)\ne: End\ns -> f\nf -> e\n"),&reg.authoring_catalog()).unwrap()
}
fn nested(target: &str) -> Blueprint {
    let mut graph = body("Add");
    let mut catalog = Registry::with_builtins().node_signatures();
    catalog.functions.insert(target.into(), FunctionEntry::derive_signature(&graph).unwrap());
    let data = serde_json::json!({"function":target});
    let signature = catalog.resolve("CallFunction", &data).unwrap();
    let old = graph.nodes[1].pins.clone();
    let node = &mut graph.nodes[1];
    node.kind = "CallFunction".into();
    node.node_type = signature.node_type;
    node.data = data;
    node.pins = signature
        .pins
        .iter()
        .map(|p| {
            let mut pin = p.instantiate(uuid::Uuid::new_v4());
            if let Some(prior) = old.iter().find(|o| {
                o.pin_type == p.pin_type
                    && (p.pin_type == PinType::ExecInput
                        || p.pin_type == PinType::ExecOutput
                        || o.name == p.name)
            }) {
                pin.id = prior.id;
            }
            pin
        })
        .collect();
    graph
}
async fn run(
    reg: Arc<Registry>,
    base: &Path,
    bp: Blueprint,
) -> crate::DaemonResult<Vec<ExecutionEvent>> {
    Interpreter::new(reg, LlmClientFactory::new(), base.into())
        .run(&Arc::new(parking_lot::RwLock::new(bp)), None)
        .await
}
fn result(events: &[ExecutionEvent], caller: uuid::Uuid) -> i64 {
    events
        .iter()
        .find_map(|e| match e {
            ExecutionEvent::NodeData {
                node_id,
                outputs,
                ..
            } if *node_id == caller => {
                outputs.iter().find_map(|(_, v)| v.as_float().map(|v| v as i64))
            }
            _ => None,
        })
        .expect("function result")
}

#[tokio::test]
async fn function_node_effects_cannot_bypass_run_approval_mode() {
    let base = temp();
    let host = host(&base);
    let p = package(&base, "com.test.nodes", &body("Calculate"), &["Calculate"], true);
    std::fs::write(p.join("main.wasm"), node_fixture::write_wasm("effect.txt")).unwrap();
    let manifest = std::fs::read_to_string(p.join("manifest.toml"))
        .unwrap()
        .replace("[addon]", "[permissions]\nrequired=[\"fs:write\"]\n[addon]");
    std::fs::write(p.join("manifest.toml"), manifest).unwrap();
    sign(&p);
    assert!(host.install(&p, Some(&base), &[]).await.is_err());
    host.install(&p, Some(&base), &["fs:write".into()]).await.unwrap();
    let registry = host.registry_for(Some(&base), false).await.unwrap();
    let graph = Arc::new(parking_lot::RwLock::new(caller(&registry, "ComTestNodesLibrary")));
    let mut config = metteur_shared::config::Config::default();
    config.sandbox.mode = "ask".into();
    let result = Interpreter::new(registry.clone(), LlmClientFactory::new(), base.clone())
        .with_config(Arc::new(tokio::sync::RwLock::new(config)))
        .run(&graph, None)
        .await;
    assert!(result.is_err(), "Ask mode must require a real approval through function frames");
    assert!(!base.join("effect.txt").exists());
    let db = crate::storage::persistence::Db::open(&base.join("journal")).unwrap();
    Interpreter::new(registry, LlmClientFactory::new(), base.clone())
        .with_workspace_db(db.clone())
        .run(&graph, None)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(base.join("effect.txt")).unwrap(), "node journal");
    assert!(!db.scan(crate::storage::persistence::cf::FILE_INTENTS).unwrap().is_empty());
}

#[tokio::test]
async fn readonly_library_real_frames_node_dependency_restart_and_scoped_removal() {
    let base = temp();
    let a = base.join("a");
    let b = base.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let host = host(&base);
    let p = package(&base, "com.test.nodes", &body("Calculate"), &["Calculate"], true);
    host.install(&p, Some(&a), &[]).await.unwrap();
    host.install(&p, Some(&b), &[]).await.unwrap();
    let ra = host.registry_for(Some(&a), false).await.unwrap();
    let rb = host.registry_for(Some(&b), false).await.unwrap();
    let name = "ComTestNodesLibrary";
    let function = ra.function(name).unwrap();
    assert_eq!(function.source, FunctionSource::Addon);
    assert_eq!(function.signature.inputs[0].name, "A");
    assert_eq!(function.signature.outputs[0].name, "Result");
    assert_ne!(function.id, rb.function(name).unwrap().id);
    let graph = caller(&ra, name);
    let events = run(ra.clone(), &a, graph.clone()).await.unwrap();
    assert_eq!(result(&events, graph.nodes[1].id), 14);
    assert!(run(rb.clone(), &b, graph.clone()).await.is_err());
    assert!(host.registry_for(None, false).await.unwrap().function(name).is_none());
    let restarted = super::tests::host(&base);
    let reread = restarted.registry_for(Some(&a), false).await.unwrap();
    assert_eq!(reread.function(name).unwrap().id, function.id);
    assert_eq!(reread.addon_function_bindings, ra.addon_function_bindings);
    host.set_enabled("com.test.nodes", false, Some(&a)).await.unwrap();
    let disabled = host.registry_for(Some(&a), false).await.unwrap();
    assert!(disabled.function(name).is_none());
    assert!(run(disabled, &a, graph.clone()).await.is_err());
    host.set_enabled("com.test.nodes", true, Some(&a)).await.unwrap();
    host.uninstall("com.test.nodes", Some(&a)).await.unwrap();
    assert!(host.registry_for(Some(&a), false).await.unwrap().function(name).is_none());
    assert!(host.registry_for(Some(&b), false).await.unwrap().function(name).is_some());
    assert_eq!(result(&run(ra, &a, graph.clone()).await.unwrap(), graph.nodes[1].id), 14);
}

#[tokio::test]
async fn dependency_upgrade_and_removal_change_the_function_binding_without_silent_retargeting() {
    let base = temp();
    let host = host(&base);
    let node = node_fixture::package(
        &base,
        "1.0.0",
        &node_fixture::signature(),
        &node_fixture::wasm(r#"{"outputs":{"result":14}}"#),
    );
    host.install(&node, None, &[]).await.unwrap();
    let p = package(
        &base,
        "com.aaa.library",
        &body("ComTestNodesCalculate"),
        &["ComTestNodesCalculate"],
        false,
    );
    host.install(&p, None, &[]).await.unwrap();
    let old = host.registry_for(Some(&base), false).await.unwrap();
    let graph = caller(&old, "ComAaaLibraryLibrary");
    assert_eq!(
        result(&run(old.clone(), &base, graph.clone()).await.unwrap(), graph.nodes[1].id),
        14
    );
    let updated = node_fixture::package(
        &base,
        "2.0.0",
        &node_fixture::signature(),
        &node_fixture::wasm(r#"{"outputs":{"result":21}}"#),
    );
    host.install(&updated, None, &[]).await.unwrap();
    let new = host.registry_for(Some(&base), false).await.unwrap();
    assert!(new.function("ComAaaLibraryLibrary").is_some());
    assert_ne!(new.addon_function_bindings, old.addon_function_bindings);
    assert!(run(new.clone(), &base, graph).await.is_err());
    let graph = caller(&new, "ComAaaLibraryLibrary");
    assert_eq!(result(&run(new, &base, graph.clone()).await.unwrap(), graph.nodes[1].id), 21);
    host.uninstall("com.test.nodes", None).await.unwrap();
    let missing = host.registry_for(Some(&base), false).await.unwrap();
    assert!(missing.function("ComAaaLibraryLibrary").is_none());
    assert!(run(missing, &base, graph).await.is_err());
}

#[tokio::test]
async fn invalid_graphs_cycles_native_effects_missing_declarations_and_conflicts_are_rejected() {
    let deep = format!("{}int{}", "list<".repeat(1000), ">".repeat(1000));
    assert!(check_types(&serde_json::json!({"nodes":[{"pins":[{"data_type":deep}]}]})).is_err());
    assert!(
        check_types(&serde_json::json!({"nodes":[{"inputs":[{"kind":"data-in","type":deep}]}]}))
            .is_err()
    );
    let base = temp();
    let host = host(&base);
    for case in 0..5 {
        let mut graph = body("Add");
        let mut deps = vec![];
        match case {
            0 => {
                graph.entry_node_id = uuid::Uuid::new_v4();
            }
            1 => {
                graph.nodes[1].kind = "Tool".into();
            }
            2 => {
                graph.nodes[1].kind = "CallFunction".into();
                graph.nodes[1].data = serde_json::json!({"function":"Library"});
                deps.push("Library");
            }
            3 => {
                graph.nodes[1].pins[0].id = graph.nodes[0].pins[0].id;
            }
            _ => deps.push("MissingFunction"),
        };
        let p = package(&base, "com.test.invalid", &graph, &deps, false);
        assert!(host.install(&p, None, &[]).await.is_err(), "{case}");
        assert!(
            host.registry_for(None, false)
                .await
                .unwrap()
                .function("ComTestInvalidLibrary")
                .is_none()
        );
    }
    let registry = Arc::new(Registry::with_builtins());
    let graph = body("Add");
    let existing = FunctionEntry {
        id: graph.id,
        name: "ComTestConflictLibrary".into(),
        description: "User-owned".into(),
        signature: FunctionEntry::derive_signature(&graph).unwrap(),
        body: graph,
        source: FunctionSource::Global,
    };
    registry.register_function(existing.clone());
    let conflicting =
        AddonHost::new(&base.join("conflict"), registry.clone(), 1000, &Default::default());
    let p = package(&base, "com.test.conflict", &body("Add"), &[], false);
    assert!(conflicting.install(&p, None, &[]).await.is_err());
    assert_eq!(registry.function(&existing.name).unwrap(), existing);
}

#[tokio::test]
async fn transitive_function_calls_resolve_in_dependency_order_and_bound_recursion() {
    let base = temp();
    let host = host(&base);
    let p = package(&base, "com.test.chain", &nested("Leaf"), &["Leaf"], false);
    let text = std::fs::read_to_string(p.join("manifest.toml")).unwrap()
        + "\n[[functions]]\nname=\"Leaf\"\nfile=\"leaf.blueprint\"\n";
    std::fs::write(p.join("manifest.toml"), text).unwrap();
    std::fs::write(
        p.join("leaf.blueprint"),
        crate::storage::blueprint_files::encode_native(&body("Add")).unwrap(),
    )
    .unwrap();
    sign(&p);
    host.install(&p, None, &[]).await.unwrap();
    let reg = host.registry_for(Some(&base), false).await.unwrap();
    let graph = caller(&reg, "ComTestChainLibrary");
    assert_eq!(result(&run(reg, &base, graph.clone()).await.unwrap(), graph.nodes[1].id), 14);
    // A cycle that spans two package functions must fail atomically.
    std::fs::write(
        p.join("leaf.blueprint"),
        crate::storage::blueprint_files::encode_native(&nested("Library")).unwrap(),
    )
    .unwrap();
    let text =
        std::fs::read_to_string(p.join("manifest.toml")).unwrap() + "dependencies=[\"Library\"]\n";
    std::fs::write(p.join("manifest.toml"), text).unwrap();
    sign(&p);
    assert!(host.install(&p, None, &[]).await.is_err());
    let deep = base.join("deep");
    std::fs::create_dir(&deep).unwrap();
    let mut manifest = "id=\"com.test.deep\"\nversion=\"1.0.0\"\nname=\"Deep\"\n".to_string();
    for i in 0..33 {
        let body = if i == 32 {
            body("Add")
        } else {
            nested(&format!("Level{}", i + 1))
        };
        std::fs::write(
            deep.join(format!("{i}.blueprint")),
            crate::storage::blueprint_files::encode_native(&body).unwrap(),
        )
        .unwrap();
        manifest.push_str(&format!(
            "[[functions]]\nname=\"Level{i}\"\nfile=\"{i}.blueprint\"\ndependencies={}\n",
            if i == 32 {
                "[]".into()
            } else {
                format!("[\"Level{}\"]", i + 1)
            }
        ));
    }
    std::fs::write(deep.join("manifest.toml"), manifest).unwrap();
    sign(&deep);
    let error = host.install(&deep, None, &[]).await.unwrap_err();
    assert!(error.to_string().contains("nesting limit"));
}

#[tokio::test]
async fn function_frame_resume_requires_unchanged_package_and_function_evidence() {
    use crate::DaemonResult;
    use crate::execution::{CheckpointSink, ExecutionCheckpoint, Interpreter};
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Sink {
        id: uuid::Uuid,
        writes: parking_lot::Mutex<Vec<ExecutionCheckpoint>>,
        pause: Option<Arc<AtomicBool>>,
    }
    impl CheckpointSink for Sink {
        fn run_id(&self) -> uuid::Uuid {
            self.id
        }
        fn write(&self, c: &ExecutionCheckpoint) -> DaemonResult<()> {
            if c.call_stack.iter().any(|f| f.function.is_some())
                && let Some(p) = &self.pause
            {
                p.store(true, Ordering::SeqCst);
            }
            self.writes.lock().push(c.clone());
            Ok(())
        }
    }
    let base = temp();
    let host = host(&base);
    let p = package(&base, "com.test.nodes", &body("Calculate"), &["Calculate"], true);
    host.install(&p, Some(&base), &[]).await.unwrap();
    let reg = host.registry_for(Some(&base), true).await.unwrap();
    let bp = Arc::new(parking_lot::RwLock::new(caller(&reg, "ComTestNodesLibrary")));
    let pause = Arc::new(AtomicBool::new(false));
    let sink = Arc::new(Sink {
        id: uuid::Uuid::new_v4(),
        writes: Default::default(),
        pause: Some(pause.clone()),
    });
    let (task_bp, task_sink, task_base) = (bp.clone(), sink.clone(), base.clone());
    let task = tokio::spawn(async move {
        Interpreter::new(reg, LlmClientFactory::new(), task_base)
            .with_checkpoint_sink(task_sink)
            .run_with_control(&task_bp, None, pause, Default::default())
            .await
    });
    for _ in 0..300 {
        if sink
            .writes
            .lock()
            .last()
            .is_some_and(|c| c.call_stack.iter().any(|f| f.function.is_some()))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    task.abort();
    let _ = task.await;
    let checkpoint = sink.writes.lock().last().unwrap().clone();
    assert!(checkpoint.view.invocations.iter().any(|i| i.finished_at.is_some()));
    let unchanged = host.registry_for(Some(&base), false).await.unwrap();
    let mut legacy = checkpoint.clone();
    legacy.function_identities = None;
    assert!(legacy.ensure_addons(&unchanged).is_err());
    let mut same = Interpreter::new(unchanged, LlmClientFactory::new(), base.clone());
    same.resume_with_control(&bp, checkpoint.clone(), None, Default::default(), Default::default())
        .await
        .unwrap();
    let p2 = package(&base, "com.test.nodes", &body("Calculate"), &["Calculate"], true);
    let text = std::fs::read_to_string(p2.join("manifest.toml")).unwrap().replace("1.0.0", "2.0.0");
    std::fs::write(p2.join("manifest.toml"), text).unwrap();
    sign(&p2);
    host.install(&p2, Some(&base), &[]).await.unwrap();
    for remove in [false, true] {
        if remove {
            host.uninstall("com.test.nodes", Some(&base)).await.unwrap();
        }
        let reg = host.registry_for(Some(&base), false).await.unwrap();
        let resumed = Arc::new(Sink {
            id: checkpoint.run_id,
            writes: Default::default(),
            pause: None,
        });
        let mut runner = Interpreter::new(reg, LlmClientFactory::new(), base.clone())
            .with_checkpoint_sink(resumed.clone());
        let error = runner
            .resume_with_control(
                &bp,
                checkpoint.clone(),
                None,
                Default::default(),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("checkpoint"));
        assert!(resumed.writes.lock().is_empty());
    }
}
