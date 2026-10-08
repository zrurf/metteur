//! Admission through AddonHost and the real rmcp protocol, with local services.
use super::*;
use crate::{execution::context::ExecutionContext, llm::LlmClientFactory, registry::Registry};
use std::path::PathBuf;
fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("r08-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn host(base: &Path) -> Arc<AddonHost> {
    AddonHost::new(
        &base.join("data"),
        Arc::new(Registry::with_builtins()),
        1000,
        &Default::default(),
    )
}

#[tokio::test]
async fn workspace_close_rejects_pending_admission_and_late_registry_requests() {
    let base = temp();
    let ws = base.join("workspace");
    std::fs::create_dir(&ws).unwrap();
    let config = base.join("isolated.toml");
    std::fs::write(&config, "").unwrap();
    let manager = crate::workspace::WorkspaceManager::new().with_global_config_path(config);
    manager.open(&ws).await.unwrap();
    let host = host(&base);
    let pending = host.registry_for(Some(&ws), true).await.unwrap();
    assert!(host.close_workspace(&ws, &manager).await.is_err());
    assert!(manager.get(&ws).await.is_some());
    drop(pending);
    host.close_workspace(&ws, &manager).await.unwrap();
    assert!(manager.get(&ws).await.is_none());
    assert!(host.registry_for(Some(&ws), true).await.is_err());
    // The existing watcher releases its database when cancellation is polled.
    tokio::task::yield_now().await;
    manager.open(&ws).await.unwrap();
    host.opened_workspace(&ws).await;
    assert!(host.registry_for(Some(&ws), false).await.is_ok());
    host.close_workspace(&ws, &manager).await.unwrap();
}
fn package(base: &Path, label: &str, mode: &str) -> PathBuf {
    let path = base.join(format!("package-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    let pid = base.join(format!("{label}.pid"));
    std::fs::write(path.join("server.py"), include_str!("test_mcp_server.py")).unwrap();
    std::fs::write(
        path.join("manifest.toml"),
        format!(
            r#"id="com.test.r08"
version="1.0.0"
name="MCP test"
[permissions]
required=["process"]
[[mcp]]
name="Local"
[mcp.server]
transport="stdio"
command=["/usr/bin/python3","${{package}}/server.py","{label}","{mode}",{}]
"#,
            serde_json::to_string(&pid.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    path
}
async fn invoke(registry: Arc<Registry>, root: &Path) -> crate::DaemonResult<Value> {
    let tool = registry.tool("ComTestR08LocalReadValue").expect("discovered actual MCP tool");
    let mut ctx = ExecutionContext::new(registry, LlmClientFactory::new(), root.into());
    tool.call(&[], &mut ctx).await
}
#[cfg(unix)]
async fn dead(path: &Path) {
    let pids = std::fs::read_to_string(path).unwrap();
    for _ in 0..100 {
        let alive = pids.split_whitespace().any(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
                !s.split(')').nth(1).unwrap_or_default().trim_start().starts_with('Z')
            })
        });
        if !alive {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("owned MCP process tree remains alive");
}
#[cfg(unix)]
#[tokio::test]
async fn real_stdio_scope_grants_upgrade_disable_uninstall_and_close() {
    let base = temp();
    let a = base.join("a");
    let b = base.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let host = host(&base);
    let pa = package(&base, "a", "normal");
    let pb = package(&base, "b", "normal");
    assert!(host.install(&pa, Some(&a), &[]).await.is_err());
    assert!(!base.join("a.pid").exists());
    host.install(&pa, Some(&a), &["process".into()]).await.unwrap();
    host.install(&pb, Some(&b), &["process".into()]).await.unwrap();
    let ra = host.registry_for(Some(&a), true).await.unwrap();
    let rb = host.registry_for(Some(&b), false).await.unwrap();
    assert_eq!(invoke(ra.clone(), &a).await.unwrap(), Value::String("a:absent:False".into()));
    assert_eq!(invoke(rb.clone(), &b).await.unwrap(), Value::String("b:absent:False".into()));
    assert!(
        host.registry_for(None, false).await.unwrap().tool("ComTestR08LocalReadValue").is_none()
    );
    assert!(host.set_enabled("com.test.r08", false, Some(&a)).await.is_err());
    drop(ra);
    let upgraded = package(&base, "a2", "normal");
    host.install(&upgraded, Some(&a), &["process".into()]).await.unwrap();
    dead(&base.join("a.pid")).await;
    let ra = host.registry_for(Some(&a), false).await.unwrap();
    assert_eq!(invoke(ra.clone(), &a).await.unwrap(), Value::String("a2:absent:False".into()));
    host.set_enabled("com.test.r08", false, Some(&a)).await.unwrap();
    dead(&base.join("a2.pid")).await;
    assert!(invoke(ra, &a).await.is_err());
    assert_eq!(invoke(rb, &b).await.unwrap(), Value::String("b:absent:False".into()));
    host.set_enabled("com.test.r08", true, Some(&a)).await.unwrap();
    host.registry_for(Some(&a), false).await.unwrap();
    host.uninstall("com.test.r08", Some(&a)).await.unwrap();
    dead(&base.join("a2.pid")).await;
    let pinned = host.registry_for(Some(&b), true).await.unwrap();
    let mut context = services::ServiceContext::default();
    context.config.servers.insert("ComTestR08Local".into(), Default::default());
    let overridden = host.registry_for_context(Some(&b), false, &context).await.unwrap();
    assert!(overridden.tool("ComTestR08LocalReadValue").is_none());
    assert_eq!(invoke(pinned.clone(), &b).await.unwrap(), Value::String("b:absent:False".into()));
    drop(pinned);
    // No further admission or management call is needed to reclaim the old process.
    dead(&base.join("b.pid")).await;
    host.close_services(&b).await;
    dead(&base.join("b.pid")).await;
}
#[cfg(unix)]
#[tokio::test]
async fn real_stdio_timeout_disconnect_persisted_deny_and_user_override() {
    let base = temp();
    let ws = base.join("ws");
    std::fs::create_dir(&ws).unwrap();
    let host = host(&base);
    let p = package(&base, "hang", "hang");
    host.install(&p, Some(&ws), &["process".into()]).await.unwrap();
    let mut context = services::ServiceContext::default();
    context.config.call_timeout_secs = 1;
    let r = host.registry_for_context(Some(&ws), false, &context).await.unwrap();
    assert!(r.tool("ComTestR08LocalReadValue").is_none());
    assert_eq!(host.mcp_statuses(Some(&ws)).await[0].status, "Failed");
    dead(&base.join("hang.pid")).await;
    let p = package(&base, "disconnect", "disconnect");
    host.install(&p, Some(&ws), &["process".into()]).await.unwrap();
    let r = host.registry_for_context(Some(&ws), false, &context).await.unwrap();
    assert!(invoke(r, &ws).await.is_err());
    host.set_enabled("com.test.r08", false, Some(&ws)).await.unwrap();
    dead(&base.join("disconnect.pid")).await;
    let p = package(&base, "denied", "normal");
    host.install(&p, Some(&ws), &["process".into()]).await.unwrap();
    let manifest = manifest::Manifest::load(&p).unwrap().0;
    let command = manifest.mcp[0].server.command.join(" ");
    let db = crate::storage::persistence::Db::open(&base.join("grants")).unwrap();
    crate::sandbox::grant::GrantStore::new(Some(db.clone()), None)
        .store(
            crate::sandbox::approval::Scope::Workspace,
            crate::sandbox::command_hash(&crate::sandbox::policy::normalize(&command)),
            crate::sandbox::approval::Decision::Deny,
            &command,
        )
        .unwrap();
    context.workspace_db = Some(db);
    assert!(
        host.registry_for_context(Some(&ws), false, &context)
            .await
            .unwrap()
            .tool("ComTestR08LocalReadValue")
            .is_none()
    );
    assert!(!base.join("denied.pid").exists());
    context
        .config
        .servers
        .insert("ComTestR08Local".into(), metteur_shared::config::McpServerConfig::default());
    host.registry_for_context(Some(&ws), false, &context).await.unwrap();
    assert_eq!(host.mcp_statuses(Some(&ws)).await[0].status, "Overridden");
    assert!(!base.join("denied.pid").exists());
}

#[tokio::test]
async fn real_http_permission_discovery_call_and_shutdown() {
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
    async fn rpc(Json(req): Json<serde_json::Value>) -> axum::response::Response {
        if req.get("id").is_none() {
            return StatusCode::ACCEPTED.into_response();
        }
        let result = match req["method"].as_str().unwrap_or_default() {
            "initialize" => {
                serde_json::json!({"protocolVersion":req["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"r08-http","version":"1"}})
            }
            "tools/list" => {
                serde_json::json!({"tools":[{"name":"read_value","description":"local","inputSchema":{"type":"object"}}]})
            }
            "tools/call" => serde_json::json!({"content":[{"type":"text","text":"http-local"}]}),
            _ => serde_json::json!({}),
        };
        Json(serde_json::json!({"jsonrpc":"2.0","id":req["id"],"result":result})).into_response()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/mcp", post(rpc))).await.unwrap();
    });
    let base = temp();
    let p = base.join("package");
    std::fs::create_dir(&p).unwrap();
    std::fs::write(
        p.join("manifest.toml"),
        format!(
            r#"id="com.test.r08"
version="1.0.0"
name="HTTP test"
[permissions]
required=["network"]
[[mcp]]
name="Local"
[mcp.server]
transport="http"
url="http://{addr}/mcp"
"#
        ),
    )
    .unwrap();
    let host = host(&base);
    assert!(host.install(&p, None, &[]).await.is_err());
    host.install(&p, None, &["network".into()]).await.unwrap();
    let r = host.registry_for(None, false).await.unwrap();
    assert_eq!(invoke(r.clone(), &base).await.unwrap(), Value::String("http-local".into()));
    host.uninstall("com.test.r08", None).await.unwrap();
    assert!(invoke(r, &base).await.is_err());
    task.abort();
}

#[cfg(unix)]
#[test]
fn environment_reference_is_explicit_and_redacted_in_real_stdio_response() {
    const FLAG: &str = "METTEUR_R08_ENV_TEST_CHILD";
    const SECRET: &str = "METTEUR_R08_SYNTHETIC_REFERENCE";
    if std::env::var_os(FLAG).is_none() {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","addon::mcp_tests::environment_reference_is_explicit_and_redacted_in_real_stdio_response"]).env(FLAG,"1").env(SECRET,"r08-synthetic-value-only").output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let base = temp();
        let p = package(&base, "environment", "normal");
        let manifest = std::fs::read_to_string(p.join("manifest.toml"))
            .unwrap()
            .replace("required=[\"process\"]", "required=[\"process\",\"environment\"]");
        std::fs::write(
            p.join("manifest.toml"),
            format!("{manifest}\n[mcp.server.env_refs]\nR08_SELECTED=\"{SECRET}\"\n"),
        )
        .unwrap();
        let host = host(&base);
        assert!(host.install(&p, None, &["process".into()]).await.is_err());
        host.install(&p, None, &["process".into(), "environment".into()]).await.unwrap();
        let r = host.registry_for(None, false).await.unwrap();
        assert_eq!(
            invoke(r, &base).await.unwrap(),
            Value::String("environment:[redacted]:False".into())
        );
        host.uninstall("com.test.r08", None).await.unwrap();
        dead(&base.join("environment.pid")).await;
        use axum::{Json,Router,routing::post,http::{HeaderMap,StatusCode},response::{IntoResponse,Redirect}};
        let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let captured=calls.clone();
        let app=Router::new().route("/mcp",post(move |headers:HeaderMap,Json(req):Json<serde_json::Value>| {
            let calls=captured.clone();
            async move {
                calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                if headers.get("authorization").and_then(|v|v.to_str().ok())!=Some("Bearer r08-synthetic-value-only") {return StatusCode::UNAUTHORIZED.into_response();}
                if req.get("id").is_none() {return StatusCode::ACCEPTED.into_response();}
                let result=match req["method"].as_str().unwrap_or_default() {
                    "initialize"=>serde_json::json!({"protocolVersion":req["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"r08-auth","version":"1"}}),
                    "tools/list"=>serde_json::json!({"tools":[{"name":"read_value","description":"local","inputSchema":{"type":"object"}}]}),
                    _=>serde_json::json!({"content":[{"type":"text","text":"r08-synthetic-value-only"}]}),
                };
                Json(serde_json::json!({"jsonrpc":"2.0","id":req["id"],"result":result})).into_response()
            }
        })).route("/redirect",post(||async {Redirect::temporary("/mcp")}));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
        let task=tokio::spawn(async move {axum::serve(listener,app).await.unwrap();});
        let manifest=format!(r#"id="com.test.r08"
version="1.0.0"
name="HTTP credentials"
[permissions]
required=["network","environment"]
[[mcp]]
name="Local"
[mcp.server]
transport="http"
url="http://{addr}/mcp"
auth_env="{SECRET}"
"#);
        std::fs::write(p.join("manifest.toml"),&manifest).unwrap();
        assert!(host.install(&p,None,&["network".into()]).await.is_err());assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),0);
        host.install(&p,None,&["network".into(),"environment".into()]).await.unwrap();
        let r=host.registry_for(None,false).await.unwrap();assert_eq!(invoke(r,&base).await.unwrap(),Value::String("[redacted]".into()));
        host.uninstall("com.test.r08",None).await.unwrap();
        let before=calls.load(std::sync::atomic::Ordering::SeqCst);
        std::fs::write(p.join("manifest.toml"),manifest.replace("/mcp\"","/redirect\"")).unwrap();
        host.install(&p,None,&["network".into(),"environment".into()]).await.unwrap();
        assert!(host.registry_for(None,false).await.unwrap().tool("ComTestR08LocalReadValue").is_none());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),before,"redirect escaped the declared endpoint");
        assert_eq!(host.list(&[]).await[0].status,"Failed");host.uninstall("com.test.r08",None).await.unwrap();task.abort();
    });
}

#[cfg(unix)]
#[tokio::test]
async fn second_service_failure_removes_all_tools_fragments_and_processes() {
    let base = temp();
    let p = package(&base, "partial", "normal");
    let manifest = std::fs::read_to_string(p.join("manifest.toml")).unwrap();
    std::fs::write(
        p.join("manifest.toml"),
        format!(
            r#"{manifest}
[[fragments]]
name="Guidance"
file="guidance.md"
[[mcp]]
name="Broken"
[mcp.server]
transport="stdio"
command=["/r08/definitely-not-an-executable"]
"#
        ),
    )
    .unwrap();
    std::fs::write(p.join("guidance.md"), "Must not be admitted").unwrap();
    let host = host(&base);
    host.install(&p, None, &["process".into()]).await.unwrap();
    let r = host.registry_for(None, false).await.unwrap();
    assert!(r.tool("ComTestR08LocalReadValue").is_none());
    assert!(r.addon_packages.is_empty());
    assert!(r.addon_fragments.is_empty());
    dead(&base.join("partial.pid")).await;
    assert_eq!(host.list(&[]).await[0].status, "Failed");
}

#[cfg(unix)]
#[tokio::test]
async fn tool_collision_is_atomic_and_override_changes_checkpoint_binding() {
    let base = temp();
    let p = package(&base, "collision", "normal");
    let host = host(&base);
    let manifest = std::fs::read_to_string(p.join("manifest.toml")).unwrap();
    std::fs::write(
        p.join("main.wasm"),
        wat::parse_str("(module (func (export \"run\") (result i32) i32.const 0))").unwrap(),
    )
    .unwrap();
    std::fs::write(p.join("manifest.toml"),format!("{manifest}\n[addon]\nentry=\"main.wasm\"\n[[tools]]\nname=\"LocalReadValue\"\nfunction=\"run\"\n")).unwrap();
    host.install(&p, None, &["process".into()]).await.unwrap();
    let r = host.registry_for(None, false).await.unwrap();
    assert!(r.tool("ComTestR08LocalReadValue").is_none());
    assert!(r.addon_packages.is_empty());
    dead(&base.join("collision.pid")).await;
    std::fs::write(p.join("manifest.toml"), manifest).unwrap();
    host.install(&p, None, &["process".into()]).await.unwrap();
    let r = host.registry_for(None, false).await.unwrap();
    let resource = r.tool("ComTestR08ListMcpResources").unwrap();
    let mut ctx = ExecutionContext::new(r.clone(), LlmClientFactory::new(), base.clone());
    assert_eq!(
        resource
            .call(&[Value::Json(serde_json::json!({"server":"ComTestR08Local"}))], &mut ctx)
            .await
            .unwrap(),
        Value::Json(serde_json::json!({"resources":[]}))
    );
    let mut checkpoint = crate::execution::checkpoint::ExecutionCheckpoint::running(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        1,
    );
    checkpoint.addon_packages = r.addon_packages.clone();
    assert!(checkpoint.ensure_addons(&host.registry_for(None, false).await.unwrap()).is_ok());
    let mut context = services::ServiceContext::default();
    context.config.servers.insert("ComTestR08Local".into(), Default::default());
    let changed = host.registry_for_context(None, false, &context).await.unwrap();
    assert!(checkpoint.ensure_addons(&changed).is_err());
    dead(&base.join("collision.pid")).await;
    host.uninstall("com.test.r08", None).await.unwrap();
}
