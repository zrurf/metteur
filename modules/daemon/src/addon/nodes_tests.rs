use super::*;
use crate::{
    addon::AddonHost, execution::ExecutionCheckpoint, llm::LlmClientFactory, registry::Registry,
};
use metteur_shared::{
    DataType, NodeType,
    node_catalog::{PinSignature, addon::BINDING_KEY},
};
use std::path::{Path, PathBuf};

fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("r11-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&p).unwrap();
    p
}

pub(in crate::addon) fn write_wasm(path: &str) -> Vec<u8> {
    fn alloc(text: &str, local: &str) -> String {
        let stores = text
            .bytes()
            .enumerate()
            .map(|(i, b)| {
                format!("local.get ${local} i64.const {i} i64.add i32.const {b} call $store")
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("i64.const {} call $alloc local.set ${local} {stores}", text.len())
    }
    let result = r#"{"outputs":{"result":14}}"#;
    wat::parse_str(format!(
        r#"(module
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output (param i64 i64)))
      (import "extism:host/user" "fs_write" (func $write (param i64 i64)))
      (func (export "run") (result i32) (local $path i64) (local $data i64) (local $result i64)
        {} {} {} local.get $path local.get $data call $write
        local.get $result i64.const {} call $output i32.const 0))"#,
        alloc(path, "path"),
        alloc("node journal", "data"),
        alloc(result, "result"),
        result.len()
    ))
    .unwrap()
}

#[tokio::test]
async fn node_file_effects_keep_grants_original_approval_journal_and_path_boundaries() {
    use crate::sandbox::PermissionMode;
    let base = temp();
    let host = host(&base);
    for (i, (path, grant, mode, allowed)) in [
        ("denied.txt", false, PermissionMode::Sandbox, false),
        ("ask.txt", true, PermissionMode::Ask, false),
        ("allowed.txt", true, PermissionMode::Sandbox, true),
        (".metteur/config.toml", true, PermissionMode::Sandbox, false),
        ("../outside-r11.txt", true, PermissionMode::Sandbox, false),
    ]
    .iter()
    .enumerate()
    {
        let p = package(&base, &format!("3.0.{i}"), &signature(), &write_wasm(path));
        if *grant {
            let manifest = std::fs::read_to_string(p.join("manifest.toml"))
                .unwrap()
                .replace("[addon]", "[permissions]\nrequired=[\"fs:write\"]\n[addon]");
            std::fs::write(p.join("manifest.toml"), manifest).unwrap();
            let key = ed25519_dalek::SigningKey::from_bytes(&[19; 32]);
            std::fs::write(
                p.join("signature.toml"),
                crate::addon::signature::sign_package(&p, &key).unwrap(),
            )
            .unwrap();
            assert!(host.install(&p, Some(&base), &[]).await.is_err());
        }
        host.install(
            &p,
            Some(&base),
            &if *grant {
                vec!["fs:write".into()]
            } else {
                vec![]
            },
        )
        .await
        .unwrap();
        let reg = host.registry_for(Some(&base), false).await.unwrap();
        let bp = graph(&reg);
        let node = &bp.nodes[1];
        let mut ctx = ExecutionContext::new(reg.clone(), LlmClientFactory::new(), base.clone());
        ctx.permission_mode = *mode;
        ctx.workspace_db = Some(
            crate::storage::persistence::Db::open(&base.join(format!("journal-{i}"))).unwrap(),
        );
        let result =
            reg.node_executor(&node.kind).unwrap().execute(node, &HashMap::new(), &mut ctx).await;
        assert_eq!(result.is_ok(), *allowed, "{path}: {result:?}");
        if *allowed {
            assert_eq!(std::fs::read_to_string(base.join(path)).unwrap(), "node journal");
            assert!(ctx.mutated_paths.contains(&PathBuf::from(path)));
            assert!(
                !ctx.workspace_db
                    .as_ref()
                    .unwrap()
                    .scan(crate::storage::persistence::cf::FILE_INTENTS)
                    .unwrap()
                    .is_empty()
            );
            ctx.transaction_log.rollback().unwrap();
            assert!(!base.join(path).exists());
        } else {
            assert!(!base.join(path).exists());
        }
    }
}
pub(in crate::addon) fn signature() -> NodeSignature {
    NodeSignature {
        kind: "Calculate".into(),
        executor_kind: "Calculate".into(),
        node_type: NodeType::Function,
        dynamic_pins: false,
        description: "Test node".into(),
        pins: vec![
            PinSignature {
                key: "in".into(),
                name: "In".into(),
                pin_type: PinType::ExecInput,
                data_type: DataType::Void,
                ..Default::default()
            },
            PinSignature {
                key: "out".into(),
                name: "Out".into(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            },
            PinSignature {
                key: "a".into(),
                name: "A".into(),
                pin_type: PinType::DataInput,
                data_type: DataType::Int,
                ..Default::default()
            },
            PinSignature {
                key: "result".into(),
                name: "Result".into(),
                pin_type: PinType::DataOutput,
                data_type: DataType::Int,
                ..Default::default()
            },
        ],
    }
}
pub(in crate::addon) fn wasm(output: &str) -> Vec<u8> {
    let input = r#"{"inputs":{"a":7}}"#;
    let check = input
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            format!("i64.const {i} call $load i32.const {b} i32.ne if i32.const 1 return end")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let stores = output
        .bytes()
        .enumerate()
        .map(|(i, b)| format!("local.get $p i64.const {i} i64.add i32.const {b} call $store"))
        .collect::<Vec<_>>()
        .join("\n");
    wat::parse_str(format!(
        r#"(module
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output (param i64 i64)))
      (import "extism:host/env" "input_length" (func $length (result i64)))
      (import "extism:host/env" "input_load_u8" (func $load (param i64) (result i32)))
      (func (export "run") (result i32) (local $p i64)
        call $length i64.const {} i64.ne if i32.const 1 return end {check}
        i64.const {} call $alloc local.set $p {stores}
        local.get $p i64.const {} call $output i32.const 0))"#,
        input.len(),
        output.len(),
        output.len()
    ))
    .unwrap()
}
pub(in crate::addon) fn package(base: &Path, version: &str, sig: &NodeSignature, bytes: &[u8]) -> PathBuf {
    let p = base.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&p).unwrap();
    std::fs::write(p.join("main.wasm"), bytes).unwrap();
    std::fs::write(p.join("manifest.toml"),format!("id=\"com.test.nodes\"\nversion=\"{version}\"\nname=\"Nodes\"\n[addon]\nentry=\"main.wasm\"\ncall_timeout_ms=40\n[[nodes]]\nname=\"Calculate\"\nfunction=\"run\"\n[nodes.signature]\n{}",toml::to_string(sig).unwrap().replace("[[pins]]", "[[nodes.signature.pins]]"))).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[19; 32]);
    std::fs::write(
        p.join("signature.toml"),
        crate::addon::signature::sign_package(&p, &key).unwrap(),
    )
    .unwrap();
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
fn graph(registry: &Registry) -> metteur_shared::Blueprint {
    metteur_shared::dsl::compile_with_catalog("blueprint \"Nodes\"\nentry s: Start\nn: ComTestNodesCalculate(A = 7)\ne: End\ns -> n\nn -> e\n",&registry.authoring_catalog()).unwrap()
}
async fn call(
    registry: Arc<Registry>,
    base: &Path,
    node: &Node,
) -> DaemonResult<HashMap<PinId, Value>> {
    let mut ctx = ExecutionContext::new(registry.clone(), LlmClientFactory::new(), base.into());
    registry.node_executor(&node.kind).unwrap().execute(node, &HashMap::new(), &mut ctx).await
}
#[tokio::test]
async fn signed_nodes_execute_real_json_and_preserve_scope_version_and_checkpoint_identity() {
    let base = temp();
    let a = base.join("a");
    let b = base.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let host = host(&base);
    let p = package(&base, "1.0.0", &signature(), &wasm(r#"{"outputs":{"result":14}}"#));
    host.install(&p, Some(&a), &[]).await.unwrap();
    let old = host.registry_for(Some(&a), false).await.unwrap();
    let bp = graph(&old);
    let node = &bp.nodes[1];
    let output = call(old.clone(), &a, node).await.unwrap();
    assert_eq!(output.values().next(), Some(&Value::Int(14)));
    assert!(
        metteur_shared::model::validate::validate_with_catalog(&bp, &old.node_signatures()).is_ok()
    );
    assert!(host.registry_for(Some(&b), false).await.unwrap().node_executor(&node.kind).is_none());
    assert!(host.registry_for(None, false).await.unwrap().node_executor(&node.kind).is_none());
    let mut checkpoint = ExecutionCheckpoint::running(uuid::Uuid::new_v4(), bp.id, 0);
    checkpoint.addon_packages = old.addon_packages.clone();
    let p2 = package(&base, "2.0.0", &signature(), &wasm(r#"{"outputs":{"result":21}}"#));
    host.install(&p2, Some(&a), &[]).await.unwrap();
    let new = host.registry_for(Some(&a), false).await.unwrap();
    assert!(checkpoint.ensure_addons(&new).is_err());
    assert!(
        !metteur_shared::model::validate::validate_with_catalog(&bp, &new.node_signatures())
            .is_ok()
    );
    assert!(call(new.clone(), &a, node).await.is_err());
    assert_eq!(call(old.clone(), &a, node).await.unwrap(), output);
    assert_eq!(
        call(new.clone(), &a, &graph(&new).nodes[1]).await.unwrap().values().next(),
        Some(&Value::Int(21))
    );
    assert!(
        metteur_shared::dsl::compile_with_catalog(
            &metteur_shared::dsl::decompile(&bp),
            &new.authoring_catalog()
        )
        .is_err()
    );
    host.set_enabled("com.test.nodes", false, Some(&a)).await.unwrap();
    assert!(host.registry_for(Some(&a), false).await.unwrap().node_executor(&node.kind).is_none());
    host.set_enabled("com.test.nodes", true, Some(&a)).await.unwrap();
    host.uninstall("com.test.nodes", Some(&a)).await.unwrap();
    let gone = host.registry_for(Some(&a), false).await.unwrap();
    assert!(
        !metteur_shared::model::validate::validate_with_catalog(&bp, &gone.node_signatures())
            .is_ok()
    );
    assert!(checkpoint.ensure_addons(&gone).is_err());
    assert!(gone.node_executor("Add").is_some());
}
#[tokio::test]
async fn node_contract_rejects_forged_names_pins_types_and_identity() {
    let base = temp();
    let host = host(&base);
    let p = package(&base, "1.0.0", &signature(), &wasm(r#"{"outputs":{"result":14}}"#));
    host.install(&p, None, &[]).await.unwrap();
    let reg = host.registry_for(None, false).await.unwrap();
    let bp = graph(&reg);
    for case in 0..7 {
        let mut bad = bp.clone();
        let node = &mut bad.nodes[1];
        match case {
            0 => node.pins[2].name = "Other".into(),
            1 => node.pins[2].data_type = DataType::String,
            2 => {
                node.pins.pop();
            }
            3 => node.pins.push(node.pins[2].clone()),
            4 => {
                node.data.as_object_mut().unwrap().remove(BINDING_KEY);
            }
            5 => node.data["a"] = serde_json::json!(true),
            _ => node.pins[2].optional = true,
        };
        assert!(
            !metteur_shared::model::validate::validate_with_catalog(&bad, &reg.node_signatures())
                .is_ok(),
            "{case}"
        );
        assert!(call(reg.clone(), &base, &bad.nodes[1]).await.is_err());
    }
    for case in 0..6 {
        let mut sig = signature();
        match case {
            0 => sig.dynamic_pins = true,
            1 => sig.pins[2].data_type = DataType::Context,
            2 => sig.pins[2].key = "../escape".into(),
            3 => sig.pins.push(sig.pins[2].clone()),
            4 => sig.node_type = NodeType::Control,
            _ => sig.executor_kind = "Add".into(),
        };
        let p = package(&base, "2.0.0", &sig, &wasm(r#"{"outputs":{"result":14}}"#));
        assert!(host.install(&p, None, &[]).await.is_err(), "{case}");
    }
    let mut registry = reg.snapshot();
    let existing = reg.node_signatures().addon_bindings["ComTestNodesCalculate"].clone();
    let verified_package = Arc::new(
        Package::load(
            &p,
            "global",
            &crate::addon::signature::Policy::from_config(&Default::default()),
        )
        .unwrap(),
    );
    let entry = &verified_package.manifest.nodes[0];
    let node = Arc::new(AddonNode::new(verified_package.clone(), entry, HashSet::new(), 100));
    assert!(registry.replace_owned_nodes("different-owner", vec![node], existing).is_err());
    let conflict = package(&base, "4.0.0", &signature(), &wasm(r#"{"outputs":{"result":14}}"#));
    let mut manifest = std::fs::read_to_string(conflict.join("manifest.toml")).unwrap();
    manifest.push_str("\n[[tools]]\nname=\"Calculate\"\nfunction=\"run\"\n");
    std::fs::write(conflict.join("manifest.toml"), manifest).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[19; 32]);
    std::fs::write(
        conflict.join("signature.toml"),
        crate::addon::signature::sign_package(&conflict, &key).unwrap(),
    )
    .unwrap();
    assert!(host.install(&conflict, None, &[]).await.is_err());
    let intact = host.registry_for(None, false).await.unwrap();
    assert!(intact.tool("ComTestNodesCalculate").is_none());
    assert_eq!(intact.addon_packages, reg.addon_packages);
    let malformed = package(&base, "5.0.0", &signature(), &wasm(r#"{"outputs":{"result":14}}"#));
    let nested = format!("{}int{}", "list<".repeat(1000), ">".repeat(1000));
    let manifest = std::fs::read_to_string(malformed.join("manifest.toml"))
        .unwrap()
        .replace("data_type = \"int\"", &format!("data_type = \"{nested}\""));
    std::fs::write(malformed.join("manifest.toml"), manifest).unwrap();
    assert!(crate::addon::manifest::Manifest::load(&malformed).is_err());
}
#[tokio::test]
async fn node_wasm_limits_capabilities_and_output_types_fail_visibly() {
    let base = temp();
    let host = host(&base);
    let spin = wat::parse_str(
        r#"(module (func (export "run") (result i32) (loop $l br $l) i32.const 0))"#,
    )
    .unwrap();
    let denied=wat::parse_str(r#"(module (import "extism:host/user" "fs_read" (func $read (param i64) (result i64))) (func (export "run") (result i32) i64.const 0 call $read drop i32.const 0))"#).unwrap();
    for (i, bytes) in [
        wasm(r#"{"outputs":{"result":"bad"}}"#),
        wasm(r#"{"outputs":{"other":14}}"#),
        wasm(r#"{"outputs":{"result":14},"permissions":["fs:write"]}"#),
        spin,
        denied,
    ]
    .iter()
    .enumerate()
    {
        let p = package(&base, &format!("1.0.{i}"), &signature(), bytes);
        host.install(&p, None, &[]).await.unwrap();
        let reg = host.registry_for(None, false).await.unwrap();
        let bp = graph(&reg);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(3), call(reg, &base, &bp.nodes[1]))
                .await
                .unwrap()
                .is_err(),
            "{i}"
        );
    }
    assert!(!base.join("escape").exists());
}

#[tokio::test]
async fn actual_checkpoint_resume_blocks_changed_or_missing_nodes_before_progress() {
    use crate::execution::{CheckpointSink, Interpreter};
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
            if c.view.invocations.iter().any(|i| i.finished_at.is_some())
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
    let p = package(&base, "1.0.0", &signature(), &wasm(r#"{"outputs":{"result":14}}"#));
    host.install(&p, Some(&base), &[]).await.unwrap();
    let reg = host.registry_for(Some(&base), true).await.unwrap();
    let bp = Arc::new(parking_lot::RwLock::new(graph(&reg)));
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
            .is_some_and(|c| c.view.invocations.iter().any(|i| i.finished_at.is_some()))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    task.abort();
    let _ = task.await;
    let checkpoint = sink.writes.lock().last().unwrap().clone();
    assert!(checkpoint.view.invocations.iter().any(|i| i.finished_at.is_some()));
    let p2 = package(&base, "2.0.0", &signature(), &wasm(r#"{"outputs":{"result":21}}"#));
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
