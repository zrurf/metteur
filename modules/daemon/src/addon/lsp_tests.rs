//! Scoped admission and actual framed LSP consumers, using immutable local fixtures.
use super::*;
use crate::{execution::context::ExecutionContext, llm::LlmClientFactory, registry::Registry};
use serde_json::json;
use std::path::PathBuf;

fn temp() -> PathBuf {
    let path = std::env::temp_dir().join(format!("r09-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn host(base: &Path) -> Arc<AddonHost> {
    AddonHost::new(
        &base.join("data"),
        Arc::new(Registry::with_builtins()),
        500,
        &Default::default(),
    )
}
fn package(base: &Path, label: &str, mode: &str) -> PathBuf {
    let path = base.join(format!("source-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("server.py"), include_str!("test_lsp_server.py")).unwrap();
    std::fs::write(
        path.join("manifest.toml"),
        format!(
            r#"id="com.test.r09"
version="1.0.0"
name="LSP fixture"
[permissions]
required=["process"]
[addon]
entry=""
call_timeout_ms=500
[[lsp]]
name="Local"
[lsp.language]
id="fixture"
extensions=["r09", "extra"]
command=["/usr/bin/python3","${{package}}/server.py","{label}","{mode}",{}]
"#,
            serde_json::to_string(&base.join(format!("{label}.pid")).to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    path
}
fn workspace(base: &Path, name: &str) -> PathBuf {
    let path = base.join(name);
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("file.r09"), "BAD").unwrap();
    path
}
async fn call(registry: Arc<Registry>, root: &Path, name: &str) -> crate::DaemonResult<Value> {
    let tool = registry.tool(name).unwrap();
    let mut ctx = ExecutionContext::new(registry, LlmClientFactory::new(), root.into());
    tool.call(
        &[Value::Json(json!({"path":"file.r09","line":0,"character":0,"timeout_ms":300}))],
        &mut ctx,
    )
    .await
}
async fn dead(path: &Path) {
    let pids = std::fs::read_to_string(path).unwrap();
    for _ in 0..100 {
        if !pids.split_whitespace().any(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
                !s.split(')').nth(1).unwrap_or_default().trim_start().starts_with('Z')
            })
        }) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("owned LSP process tree remains alive");
}
async fn check(registry: Arc<Registry>, root: &Path) -> bool {
    use crate::registry::NodeExecutor;
    use metteur_shared::{DataType, Node, NodeType, Pin, PinType};
    let passed = uuid::Uuid::new_v4();
    let node = Node {
        id: uuid::Uuid::new_v4(),
        node_type: NodeType::Control,
        kind: "LspCheck".into(),
        position: (0., 0.),
        pins: vec![
            Pin::data("Passed", PinType::DataOutput, DataType::Bool, passed),
            Pin::data("Diagnostics", PinType::DataOutput, DataType::String, uuid::Uuid::new_v4()),
        ],
        data: json!({"path":"file.r09"}),
    };
    let mut ctx = ExecutionContext::new(registry, LlmClientFactory::new(), root.into());
    let outputs = crate::execution::nodes::LspCheckExecutor
        .execute(&node, &Default::default(), &mut ctx)
        .await
        .unwrap();
    outputs[&passed] == Value::Bool(true)
}

#[tokio::test]
async fn real_protocol_tools_node_scopes_upgrade_disable_uninstall_and_close() {
    let base = temp();
    let a = workspace(&base, "a");
    let b = workspace(&base, "b");
    let host = host(&base);
    let pa = package(&base, "a", "normal");
    let pb = package(&base, "b", "normal");
    assert!(host.install(&pa, Some(&a), &[]).await.is_err());
    assert!(!base.join("a.pid").exists());
    host.install(&pa, Some(&a), &["process".into()]).await.unwrap();
    host.install(&pb, Some(&b), &["process".into()]).await.unwrap();
    let ra = host.registry_for(Some(&a), true).await.unwrap();
    let rb = host.registry_for(Some(&b), false).await.unwrap();
    assert_eq!(
        call(ra.clone(), &a, "GetHover").await.unwrap(),
        Value::Json(json!({"content":"a:absent:False","range":null}))
    );
    let definition = call(ra.clone(), &a, "FindDefinition").await.unwrap();
    assert!(format!("{definition:?}").contains("file.r09"));
    let diagnostics = call(ra.clone(), &a, "CheckDiagnostics").await.unwrap();
    assert!(format!("{diagnostics:?}").contains("a"));
    assert!(!check(ra.clone(), &a).await);
    std::fs::write(a.join("file.r09"), "fixed").unwrap();
    assert!(check(ra.clone(), &a).await, "fresh diagnostics replace old errors");
    assert!(
        format!("{:?}", call(rb.clone(), &b, "GetHover").await.unwrap()).contains("b:absent:False")
    );
    assert!(host.set_enabled("com.test.r09", false, Some(&a)).await.is_err());
    drop(ra);
    let upgraded = package(&base, "a2", "normal");
    host.install(&upgraded, Some(&a), &["process".into()]).await.unwrap();
    dead(&base.join("a.pid")).await;
    let ra = host.registry_for(Some(&a), false).await.unwrap();
    assert!(format!("{:?}", call(ra.clone(), &a, "GetHover").await.unwrap()).contains("a2:"));
    host.set_enabled("com.test.r09", false, Some(&a)).await.unwrap();
    dead(&base.join("a2.pid")).await;
    assert!(call(ra, &a, "GetHover").await.is_err());
    host.set_enabled("com.test.r09", true, Some(&a)).await.unwrap();
    host.registry_for(Some(&a), false).await.unwrap();
    host.uninstall("com.test.r09", Some(&a)).await.unwrap();
    dead(&base.join("a2.pid")).await;
    assert!(!check(rb.clone(), &b).await);
    host.close_services(&b).await;
    dead(&base.join("b.pid")).await;
    assert!(call(rb, &b, "GetHover").await.is_err());
}

#[tokio::test]
async fn overrides_binding_conflicts_and_pinned_retirement() {
    let base = temp();
    let ws = workspace(&base, "ws");
    let host = host(&base);
    let p = package(&base, "original", "normal");
    host.install(&p, Some(&ws), &["process".into()]).await.unwrap();
    let pinned = host.registry_for(Some(&ws), true).await.unwrap();
    let mut checkpoint = crate::execution::checkpoint::ExecutionCheckpoint::running(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        1,
    );
    checkpoint.addon_packages = pinned.addon_packages.clone();
    let mut context = services::ServiceContext::default();
    context.lsp_config.languages.push(metteur_shared::config::LspLanguageConfig {
        id: "fixture".into(),
        command: vec!["/not/started".into()],
        extensions: vec!["r09".into()],
    });
    let overridden = host.registry_for_context(Some(&ws), false, &context).await.unwrap();
    assert!(overridden.addon_lsp.is_empty());
    assert!(checkpoint.ensure_addons(&overridden).is_err());
    assert!(call(pinned.clone(), &ws, "GetHover").await.is_ok());
    let mut live_config = context.lsp_config.clone();
    live_config.enabled = true;
    let live_user = crate::integration::lsp::LspManager::new(&live_config, &ws).unwrap();
    let mut active_context =
        ExecutionContext::new(pinned.clone(), LlmClientFactory::new(), ws.clone())
            .with_lsp(live_user);
    assert!(
        pinned
            .tool("GetHover")
            .unwrap()
            .call(
                &[Value::Json(json!({"path":"file.r09","line":0,"character":0}))],
                &mut active_context
            )
            .await
            .is_ok(),
        "live user reload must not replace an admitted addon route"
    );
    drop(active_context);
    drop(pinned);
    dead(&base.join("original.pid")).await;
    // Reserving only one extension leaves the other extension available.
    context.lsp_config.languages[0].id = "user".into();
    let partial = host.registry_for_context(Some(&ws), false, &context).await.unwrap();
    let manager = &partial.addon_lsp[0];
    assert_eq!(manager.language_for_extension("r09"), None);
    assert_eq!(manager.language_for_extension("EXTRA"), Some("fixture".into()));
    let collision = package(&base, "collision", "normal");
    let manifest = std::fs::read_to_string(collision.join("manifest.toml"))
        .unwrap()
        .replace("com.test.r09", "com.test.conflict");
    std::fs::write(collision.join("manifest.toml"), manifest).unwrap();
    assert!(host.install(&collision, Some(&ws), &["process".into()]).await.is_err());
    host.close_services(&ws).await;
    dead(&base.join("original.pid")).await;
}

#[tokio::test]
async fn startup_timeout_crash_request_timeout_and_diagnostics_timeout_fail_closed() {
    let base = temp();
    let ws = workspace(&base, "ws");
    let host = host(&base);
    for mode in ["hang_init", "oversize", "crash", "hang_hover", "silent"] {
        let p = package(&base, mode, mode);
        host.install(&p, Some(&ws), &["process".into()]).await.unwrap();
        if matches!(mode, "hang_init" | "oversize") {
            assert!(host.registry_for(Some(&ws), true).await.is_err());
        } else {
            let registry = host.registry_for(Some(&ws), false).await.unwrap();
            if mode == "hang_hover" {
                assert!(call(registry.clone(), &ws, "GetHover").await.is_err());
            } else {
                assert!(call(registry.clone(), &ws, "CheckDiagnostics").await.is_err());
            }
            assert!(!check(registry, &ws).await);
        }
        let status = host.list(std::slice::from_ref(&ws)).await;
        assert_eq!(status[0].status, "Failed");
        dead(&base.join(format!("{mode}.pid"))).await;
        host.set_enabled("com.test.r09", false, Some(&ws)).await.unwrap();
    }
}

#[test]
fn invalid_language_matching_commands_and_capabilities_are_rejected() {
    let base = temp();
    let p = package(&base, "schema", "normal");
    let original = std::fs::read_to_string(p.join("manifest.toml")).unwrap();
    assert!(manifest::Manifest::load(&p).is_ok());
    for body in [
        original.replace("required=[\"process\"]", "required=[]"),
        original.replace("\"r09\", \"extra\"", "\"r09\", \"R09\""),
        original.replace("extensions=[\"r09\", \"extra\"]", "extensions=[\"../r09\"]"),
        original.replace("/usr/bin/python3", "python3"),
        format!("{original}\n[lsp.language.env_refs]\nTOKEN=\"UNDECLARED_REFERENCE\"\n"),
    ] {
        std::fs::write(p.join("manifest.toml"), body).unwrap();
        assert!(manifest::Manifest::load(&p).is_err());
    }
    assert!(!base.join("schema.pid").exists());
}

#[tokio::test]
async fn persisted_deny_and_revoked_receipt_never_silently_skip_lsp_check() {
    let base = temp();
    let ws = workspace(&base, "ws");
    let host = host(&base);
    let p = package(&base, "denied", "normal");
    host.install(&p, Some(&ws), &["process".into()]).await.unwrap();
    let command = manifest::Manifest::load(&p).unwrap().0.lsp[0].language.command.join(" ");
    let db = crate::storage::persistence::Db::open(&base.join("grants")).unwrap();
    crate::sandbox::grant::GrantStore::new(Some(db.clone()), None)
        .store(
            crate::sandbox::approval::Scope::Workspace,
            crate::sandbox::command_hash(&crate::sandbox::policy::normalize(&command)),
            crate::sandbox::approval::Decision::Deny,
            &command,
        )
        .unwrap();
    let context = services::ServiceContext {
        workspace_db: Some(db),
        ..Default::default()
    };
    assert!(host.registry_for_context(Some(&ws), true, &context).await.is_err());
    assert!(!base.join("denied.pid").exists());
    let registry = host.registry_for(Some(&ws), false).await.unwrap();
    assert!(call(registry, &ws, "GetHover").await.is_ok());
    for entry in std::fs::read_dir(base.join("data/addon-state")).unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "toml") {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            let mut receipt: toml::Value = toml::from_str(&text).unwrap();
            receipt["granted"] = toml::Value::Array(vec![]);
            std::fs::write(entry.path(), toml::to_string(&receipt).unwrap()).unwrap();
        }
    }
    assert!(host.registry_for(Some(&ws), true).await.is_err());
    dead(&base.join("denied.pid")).await;
}

#[test]
fn explicit_environment_is_redacted_and_user_server_routes_first() {
    const FLAG: &str = "METTEUR_R09_TEST_CHILD";
    const SECRET: &str = "METTEUR_R09_SYNTHETIC_REFERENCE";
    if std::env::var_os(FLAG).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "addon::lsp_tests::explicit_environment_is_redacted_and_user_server_routes_first",
            ])
            .env(FLAG, "1")
            .env(SECRET, "r09-synthetic-value-only")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let base = temp();
        let ws = workspace(&base, "ws");
        let host = host(&base);
        let p = package(&base, "env", "normal");
        let manifest = std::fs::read_to_string(p.join("manifest.toml"))
            .unwrap()
            .replace("required=[\"process\"]", "required=[\"process\",\"environment\"]");
        std::fs::write(
            p.join("manifest.toml"),
            format!("{manifest}\n[lsp.language.env_refs]\nR09_TOKEN=\"{SECRET}\"\n"),
        )
        .unwrap();
        assert!(host.install(&p, Some(&ws), &["process".into()]).await.is_err());
        host.install(&p, Some(&ws), &["process".into(), "environment".into()]).await.unwrap();
        let registry = host.registry_for(Some(&ws), false).await.unwrap();
        assert_eq!(
            call(registry.clone(), &ws, "GetHover").await.unwrap(),
            Value::Json(json!({"content":"env:[redacted]:False","range":null}))
        );
        let config = metteur_shared::config::LspConfig {
            enabled: true,
            languages: vec![metteur_shared::config::LspLanguageConfig {
                id: "user".into(),
                extensions: vec!["r09".into()],
                command: vec![
                    "/usr/bin/python3".into(),
                    p.join("server.py").to_string_lossy().into_owned(),
                    "user".into(),
                    "normal".into(),
                    base.join("user.pid").to_string_lossy().into_owned(),
                ],
            }],
            ..Default::default()
        };
        let manager = crate::integration::lsp::LspManager::new(&config, &ws).unwrap();
        let registry = host
            .registry_for_context(
                Some(&ws),
                false,
                &services::ServiceContext {
                    lsp_config: config.clone(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut ctx = ExecutionContext::new(registry.clone(), LlmClientFactory::new(), ws.clone())
            .with_lsp(manager.clone());
        let response = registry
            .tool("GetHover")
            .unwrap()
            .call(&[Value::Json(json!({"path":"file.r09","line":0,"character":0}))], &mut ctx)
            .await
            .unwrap();
        assert!(format!("{response:?}").contains("user:"));
        manager.shutdown().await;
        dead(&base.join("user.pid")).await;
        host.uninstall("com.test.r09", Some(&ws)).await.unwrap();
        dead(&base.join("env.pid")).await;
    });
}

#[tokio::test]
async fn original_interpreter_retries_real_lsp_failure_and_rolls_back_the_segment() {
    use metteur_shared::{Blueprint, DataType, Edge, Node, NodeType, Pin, PinType};
    struct WriteAttempt(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl crate::registry::Tool for WriteAttempt {
        fn name(&self) -> &str {
            "WriteAttempt"
        }
        fn description(&self) -> &str {
            "isolated deterministic retry fixture"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object"})
        }
        async fn call(
            &self,
            _: &[Value],
            ctx: &mut ExecutionContext,
        ) -> crate::DaemonResult<Value> {
            let path = ctx.workspace_root.join("file.r09");
            let content = if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                b"BAD".to_vec()
            } else {
                b"fixed".to_vec()
            };
            ctx.transaction_log.record_file_write(
                path.clone(),
                std::fs::read(&path).ok(),
                content.clone(),
            );
            std::fs::write(path, content)?;
            Ok(Value::Bool(true))
        }
    }
    let base = temp();
    let ws = workspace(&base, "ws");
    let host = host(&base);
    let p = package(&base, "retry", "normal");
    host.install(&p, Some(&ws), &["process".into()]).await.unwrap();
    let registry = host.registry_for(Some(&ws), true).await.unwrap();
    let tool = Arc::new(WriteAttempt(Default::default()));
    registry.try_register_tool(tool.clone()).unwrap();
    let mut nodes = vec![];
    for (kind, node_type, data) in [
        ("Start", NodeType::Event, json!({})),
        ("Tool", NodeType::Function, json!({"tool_name":"WriteAttempt"})),
        ("LspCheck", NodeType::Control, json!({"path":"file.r09","retry":{"max_attempts":2}})),
        ("End", NodeType::Event, json!({})),
    ] {
        let mut pins = vec![];
        if kind != "Start" {
            pins.push(Pin::exec(PinType::ExecInput, uuid::Uuid::new_v4()));
        }
        if kind != "End" {
            pins.push(Pin::exec(PinType::ExecOutput, uuid::Uuid::new_v4()));
        }
        if kind == "Tool" {
            pins.push(Pin::data(
                "Result",
                PinType::DataOutput,
                DataType::Any,
                uuid::Uuid::new_v4(),
            ));
        }
        if kind == "LspCheck" {
            pins.push(Pin::data(
                "Passed",
                PinType::DataOutput,
                DataType::Bool,
                uuid::Uuid::new_v4(),
            ));
            pins.push(Pin::data(
                "Diagnostics",
                PinType::DataOutput,
                DataType::String,
                uuid::Uuid::new_v4(),
            ));
        }
        nodes.push(Node {
            id: uuid::Uuid::new_v4(),
            node_type,
            kind: kind.into(),
            position: (0., 0.),
            pins,
            data,
        });
    }
    let edges = nodes
        .windows(2)
        .map(|pair| Edge {
            id: uuid::Uuid::new_v4(),
            source_node: pair[0].id,
            source_pin: pair[0].pins.iter().find(|p| p.pin_type == PinType::ExecOutput).unwrap().id,
            target_node: pair[1].id,
            target_pin: pair[1].pins.iter().find(|p| p.pin_type == PinType::ExecInput).unwrap().id,
        })
        .collect();
    let check_id = nodes[2].id;
    let blueprint = Blueprint {
        id: uuid::Uuid::new_v4(),
        name: "R09 real retry".into(),
        entry_node_id: nodes[0].id,
        nodes,
        edges,
    };
    let mut interpreter =
        crate::execution::Interpreter::new(registry, LlmClientFactory::new(), ws.clone());
    let events =
        interpreter.run(&Arc::new(parking_lot::RwLock::new(blueprint)), None).await.unwrap();
    assert_eq!(tool.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(events.iter().any(|e|matches!(e,crate::execution::ExecutionEvent::Message{message,..} if message.contains("rolled back 1 file mutation"))));
    assert!(interpreter.attempt_count(check_id).is_none());
    assert_eq!(std::fs::read_to_string(ws.join("file.r09")).unwrap(), "fixed");
    drop(interpreter);
    host.close_services(&ws).await;
    dead(&base.join("retry.pid")).await;
}
