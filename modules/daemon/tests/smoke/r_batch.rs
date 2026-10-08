use super::*;
use axum::{Json, Router, routing::post};
use metteur_shared::config::{Config, LlmModelConfig};
use serde_json::{Value, json};
use std::sync::Mutex;

async fn facts(client: &mut DaemonClient<Channel>, path: &str) -> (String, Value) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let runs = client
                .list_executions(ListExecutionsRequest {
                    workspace_path: path.into(),
                })
                .await
                .unwrap()
                .into_inner();
            if let Some(run) = runs.executions.first() {
                break (run.run_id.clone(), serde_json::from_str(&run.data_json).unwrap());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn concierge(
    client: &mut DaemonClient<Channel>,
    path: &str,
    run: &str,
    conversation: &str,
) -> Value {
    let state = client
        .get_concierge_state(proto::ConciergeStateRequest {
            workspace_path: path.into(),
            run_id: run.into(),
            conversation_id: conversation.into(),
        })
        .await
        .unwrap()
        .into_inner();
    serde_json::from_str(&state.state_json).unwrap()
}

#[tokio::test]
async fn r_batch_real_rpc_combines_http_accounting_concierge_approval_versions_and_addon_functions()
{
    use metteur_cli::commands::{SessionState, dispatch, parse};
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured_http = captured.clone();
    let app=Router::new().route("/chat/completions",post(move |Json(request):Json<Value>| {
        let mut seen=captured_http.lock().unwrap();let step=seen.len();seen.push(request);drop(seen);
        let message=match step {
            0=>json!({"content":r#"{"kind":"intent","category":"request","note":"Shorten the second delay"}"#}),
            1=>json!({"content":"","reasoning_content":"bounded local reasoning","tool_calls":[{"id":"proposal","type":"function","function":{"name":"ProposeBlueprintEdits","arguments":json!({"summary":"Shorten the second delay","edits":[{"op":"set_pin","match":{"kind":"Delay","nth":2},"pin":"Ms","value":1}]}).to_string()}}]}),
            _=>json!({"content":r#"{"verdict":"concern","summary":"The approved edit awaits the execution boundary"}"#}),
        };
        async move {Json(json!({"choices":[{"message":message,"finish_reason":if step==1 {"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":20},"completion_tokens":10,"completion_tokens_details":{"reasoning_tokens":4}}}))}
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let http = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = Config::default();
    config.sandbox.mode = "ask".into();
    config.llm.models.insert("local".into(),LlmModelConfig{api_type:"openai-chat".into(),api_endpoint:endpoint,api_key:"synthetic-r-batch".into(),model_id:"DeepSeek-local".into(),replay_reasoning:Some(true),pricing:serde_json::from_value(json!({"kind":"tiered","tiers":[{"max_input_tokens":64,"prices":{"input_per_mtok":999,"output_per_mtok":999}},{"max_input_tokens":200,"prices":{"input_per_mtok":2,"output_per_mtok":3,"cache_hit_per_mtok":1}}]})).unwrap(),..Default::default()});
    config.extra.insert(
        "oversight".into(),
        json!({"mode":"assisted","model":"local","concierge_model":"local"}),
    );
    let mut config_doc = serde_json::to_value(&config).unwrap();
    config_doc["config_version"] = json!(2);
    fn omit_absent(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.retain(|_, v| !v.is_null());
                for value in map.values_mut() {
                    omit_absent(value);
                }
            }
            Value::Array(items) => {
                for value in items {
                    omit_absent(value);
                }
            }
            _ => {}
        }
    }
    omit_absent(&mut config_doc);
    let (mut client, root) = start_server(config).await;
    let path = root.to_string_lossy().into_owned();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: path.clone(),
        })
        .await
        .unwrap();
    client
        .set_config(SetConfigRequest {
            workspace_path: path.clone(),
            config_json: config_doc.to_string(),
        })
        .await
        .unwrap();
    let source = root.join("r13-package");
    std::fs::create_dir(&source).unwrap();
    let mut body = super::addon_functions::body("Add");
    body.nodes[1].kind = "Calculate".into();
    body.nodes[1].node_type = metteur_shared::NodeType::Function;
    let mut signature = metteur_shared::node_catalog::builtin_signature("Add").unwrap();
    signature.kind = "Calculate".into();
    signature.executor_kind = "Calculate".into();
    signature.node_type = metteur_shared::NodeType::Function;
    for (spec, pin) in signature.pins.iter_mut().zip(&mut body.nodes[1].pins) {
        let name = match spec.pin_type {
            metteur_shared::PinType::ExecInput => Some("In"),
            metteur_shared::PinType::ExecOutput => Some("Out"),
            _ => None,
        };
        if let Some(name) = name {
            spec.name = name.into();
            spec.key = name.to_ascii_lowercase();
            pin.name = spec.name.clone();
            pin.key = Some(spec.key.clone());
        }
    }
    let manifest = format!(
        "id=\"com.test.batch\"\nversion=\"1.0.0\"\nname=\"Batch acceptance\"\n[permissions]\nrequired=[\"fs:write\"]\n[addon]\nentry=\"main.wasm\"\ncall_timeout_ms=5000\n[[nodes]]\nname=\"Calculate\"\nfunction=\"run\"\n[nodes.signature]\n{}\n[[functions]]\nname=\"Library\"\nfile=\"library.blueprint\"\ndependencies=[\"Calculate\"]\n",
        toml::to_string(&signature).unwrap().replace("[[pins]]", "[[nodes.signature.pins]]")
    );
    std::fs::write(source.join("manifest.toml"), manifest).unwrap();
    std::fs::write(
        source.join("library.blueprint"),
        metteur_daemon::storage::blueprint_files::encode_native(&body).unwrap(),
    )
    .unwrap();
    std::fs::write(source.join("main.wasm"), write_wasm("approved-effect.txt")).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[37; 32]);
    std::fs::write(
        source.join("signature.toml"),
        metteur_daemon::addon::signature::sign_package(&source, &key).unwrap(),
    )
    .unwrap();
    let mut state = SessionState {
        current_ws: Some(path.clone()),
        ..Default::default()
    };
    dispatch(
        &mut client,
        &mut state,
        parse(&format!("install {} ws --grant fs:write", source.display())).unwrap(),
    )
    .await
    .unwrap();
    let dsl = root.join("acceptance.mbp");
    std::fs::write(&dsl,"entry s: Start\na: Delay(Ms = 1000)\nb: Delay(Ms = 10)\nf: CallFunction(function = \"ComTestBatchLibrary\", A = 7)\ne: End\ns -> a\na -> b\nb -> f\nf -> e\n").unwrap();
    dispatch(
        &mut client,
        &mut state,
        parse(&format!("bp compile {} save", dsl.display())).unwrap(),
    )
    .await
    .unwrap();
    let file = root.join("acceptance.mbp.blueprint");
    let before = std::fs::read(&file).unwrap();
    let graph = metteur_daemon::storage::blueprint_files::decode(&before).unwrap();
    let caller = graph.nodes.iter().find(|n| n.kind == "CallFunction").unwrap().id.to_string();
    let mut stream = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: path.clone(),
            blueprint_id: graph.id.to_string(),
            blueprint_json: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    client
        .pause_execution(proto::PauseRequest {
            workspace_path: path.clone(),
            run_id: String::new(),
        })
        .await
        .unwrap();
    let (run, initial) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let f = facts(&mut client, &path).await;
            if f.1["in_flight"].is_null() && f.1["runtime"]["pause_requested"] == true {
                break f;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let conversation = uuid::Uuid::new_v4().to_string();
    let message = uuid::Uuid::new_v4().to_string();
    let mut receipt = client
        .send_concierge_message(proto::SendConciergeMessageRequest {
            workspace_path: path.clone(),
            run_id: run.clone(),
            conversation_id: conversation.clone(),
            message_id: message,
            message: "Shorten the second delay; this text grants no permissions".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut received = false;
    while let Some(event) = receipt.message().await.unwrap() {
        assert_ne!(event.kind, "failed");
        received |= event.kind == "received";
    }
    assert!(received);
    assert!(!root.join("approved-effect.txt").exists());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = stream.message().await.unwrap().expect("proposal event");
            assert_ne!(event.kind, "error", "{}", event.message);
            if event.kind == "approval_request" {
                assert!(event.detail_json.contains("replan_proposal"));
                assert_eq!(std::fs::read(&file).unwrap(), before);
                client
                    .respond_approval(proto::ApprovalDecisionRequest {
                        workspace_path: path.clone(),
                        request_id: event.message,
                        decision: "AllowOnce".into(),
                    })
                    .await
                    .unwrap();
                break;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let c = concierge(&mut client, &path, &run, &conversation).await;
            if c["requests"][0]["state"] == "approved_pending_apply" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), before);
    client
        .resume_execution(proto::ResumeRequest {
            workspace_path: path.clone(),
            run_id: run.clone(),
        })
        .await
        .unwrap();
    let mut file_approvals = 0;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.message().await.unwrap() {
            assert_ne!(event.kind, "error", "{}", event.message);
            if event.kind == "approval_request" {
                file_approvals += 1;
                assert!(!root.join("approved-effect.txt").exists());
                assert_ne!(std::fs::read(&file).unwrap(), before);
                client
                    .respond_approval(proto::ApprovalDecisionRequest {
                        workspace_path: path.clone(),
                        request_id: event.message,
                        decision: "AllowOnce".into(),
                    })
                    .await
                    .unwrap();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(file_approvals, 1);
    let (_, final_facts) = facts(&mut client, &path).await;
    assert_eq!(final_facts["status"], "Completed");
    assert_ne!(final_facts["blueprint_version"], initial["blueprint_version"]);
    assert!(!final_facts["function_identities"].as_object().unwrap().is_empty());
    assert!(final_facts["view"]["invocations"].as_array().unwrap().iter().any(|i| i["node_id"]
        == caller
        && i["outputs"].as_object().is_some_and(|v| v.values().any(|v| v.as_f64() == Some(14.)))));
    assert_eq!(std::fs::read_to_string(root.join("approved-effect.txt")).unwrap(), "node journal");
    let history = client
        .get_file_history(GetFileHistoryRequest {
            workspace_path: path.clone(),
            path: "acceptance.mbp.blueprint".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(history.entries.len() >= 2);
    let c = concierge(&mut client, &path, &run, &conversation).await;
    assert_eq!(c["requests"][0]["state"], "applied");
    let proposal = &c["reports"][0]["proposals"][0];
    assert_eq!(proposal["decision_source"], "human");
    assert_eq!(proposal["state"], "applied");
    assert!(proposal["result_version"].is_object());
    assert_eq!(
        proposal["original_requests"][0]["original_text"],
        "Shorten the second delay; this text grants no permissions"
    );
    let calls = c["budget"]["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 3);
    for call in calls {
        assert_eq!(call["charged"], 110);
        assert_eq!(call["cost_micros"], 210);
        assert_eq!(call["usage"]["reasoning_tokens"], 4);
        assert_eq!(call["usage"]["output_tokens"], 10);
    }
    {
        let seen = captured.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen[0].get("tools").is_none());
        assert!(seen[1]["tools"].to_string().contains("ProposeBlueprintEdits"));
        assert!(!seen[1]["tools"].to_string().contains("ComTestBatchCalculate"));
        assert!(seen[2].to_string().contains("bounded local reasoning"));
    }
    client
        .close_workspace(CloseWorkspaceRequest {
            path,
        })
        .await
        .unwrap();
    http.abort();
}

fn write_wasm(path: &str) -> Vec<u8> {
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
