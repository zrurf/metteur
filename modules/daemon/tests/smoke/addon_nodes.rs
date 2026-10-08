use super::*;

#[tokio::test]
async fn scoped_catalog_cli_dsl_save_run_and_uninstall_use_the_same_node_contract() {
    use metteur_cli::commands::{Outcome, SessionState, dispatch, parse};
    let (mut client, root) = start_server(Default::default()).await;
    let path = root.to_string_lossy().into_owned();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: path.clone(),
        })
        .await
        .unwrap();
    let source = root.join("r11-source");
    std::fs::create_dir(&source).unwrap();
    let signature = metteur_shared::node_catalog::NodeSignature {
        kind: "Calculate".into(),
        executor_kind: "Calculate".into(),
        node_type: metteur_shared::NodeType::Function,
        dynamic_pins: false,
        description: "R11 real node".into(),
        pins: vec![
            metteur_shared::node_catalog::PinSignature {
                key: "in".into(),
                name: "In".into(),
                pin_type: metteur_shared::PinType::ExecInput,
                data_type: metteur_shared::DataType::Void,
                ..Default::default()
            },
            metteur_shared::node_catalog::PinSignature {
                key: "out".into(),
                name: "Out".into(),
                pin_type: metteur_shared::PinType::ExecOutput,
                data_type: metteur_shared::DataType::Void,
                ..Default::default()
            },
            metteur_shared::node_catalog::PinSignature {
                key: "a".into(),
                name: "A".into(),
                pin_type: metteur_shared::PinType::DataInput,
                data_type: metteur_shared::DataType::Int,
                ..Default::default()
            },
            metteur_shared::node_catalog::PinSignature {
                key: "result".into(),
                name: "Result".into(),
                pin_type: metteur_shared::PinType::DataOutput,
                data_type: metteur_shared::DataType::Int,
                ..Default::default()
            },
        ],
    };
    std::fs::write(source.join("manifest.toml"),format!("id=\"com.test.nodesrpc\"\nversion=\"1.0.0\"\nname=\"RPC nodes\"\n[addon]\nentry=\"main.wasm\"\n[[nodes]]\nname=\"Calculate\"\nfunction=\"run\"\n[nodes.signature]\n{}",toml::to_string(&signature).unwrap().replace("[[pins]]", "[[nodes.signature.pins]]"))).unwrap();
    let output = br#"{"outputs":{"result":14}}"#;
    let stores = output
        .iter()
        .enumerate()
        .map(|(i, b)| format!("local.get $p i64.const {i} i64.add i32.const {b} call $store"))
        .collect::<Vec<_>>()
        .join("\n");
    let bytes=wat::parse_str(format!(r#"(module
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output (param i64 i64)))
      (func (export "run") (result i32) (local $p i64) i64.const {} call $alloc local.set $p {stores} local.get $p i64.const {} call $output i32.const 0))"#,output.len(),output.len())).unwrap();
    std::fs::write(source.join("main.wasm"), bytes).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[23; 32]);
    std::fs::write(
        source.join("signature.toml"),
        metteur_daemon::addon::signature::sign_package(&source, &key).unwrap(),
    )
    .unwrap();
    let mut state = SessionState {
        current_ws: Some(path.clone()),
        ..Default::default()
    };
    dispatch(&mut client, &mut state, parse(&format!("install {} ws", source.display())).unwrap())
        .await
        .unwrap();
    let Outcome::Printed(catalog) =
        dispatch(&mut client, &mut state, parse("nodes").unwrap()).await.unwrap()
    else {
        panic!("CLI catalog")
    };
    assert!(catalog.contains("ComTestNodesrpcCalculate"));
    assert!(
        !client
            .list_node_kinds(proto::RegistryRequest::default())
            .await
            .unwrap()
            .into_inner()
            .kinds
            .contains(&"ComTestNodesrpcCalculate".into())
    );
    let info = client
        .list_node_kinds(proto::RegistryRequest {
            workspace_path: path.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .infos
        .into_iter()
        .find(|i| i.kind == "ComTestNodesrpcCalculate")
        .unwrap();
    assert!(!info.addon_binding_json.is_empty());
    assert_eq!(info.pins.len(), 4);
    let dsl = root.join("r11.mbp");
    std::fs::write(&dsl,"blueprint \"Addon node\"\nentry s: Start\nn: ComTestNodesrpcCalculate(A = 7)\ne: End\ns -> n\nn -> e\n").unwrap();
    dispatch(
        &mut client,
        &mut state,
        parse(&format!("bp compile {} save", dsl.display())).unwrap(),
    )
    .await
    .unwrap();
    let saved = metteur_daemon::storage::blueprint_files::decode(
        &std::fs::read(root.join("r11.mbp.blueprint")).unwrap(),
    )
    .unwrap();
    let loaded = client
        .load_blueprint(proto::LoadBlueprintRequest {
            workspace_path: path.clone(),
            blueprint_id: saved.id.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&loaded.nodes[1].data_json).unwrap()["_addon_binding"],
        serde_json::from_str::<serde_json::Value>(&info.addon_binding_json).unwrap()
    );
    let mut invalid = loaded.clone();
    invalid.nodes[1].pins[2].data_type = "string".into();
    assert!(
        client
            .save_blueprint(SaveBlueprintRequest {
                workspace_path: path.clone(),
                blueprint: Some(invalid),
                file_path: "bad.blueprint".into(),
                file_json: String::new()
            })
            .await
            .is_err()
    );
    assert!(!root.join("bad.blueprint").exists());
    let mut stream = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: path.clone(),
            blueprint_id: loaded.id.clone(),
            blueprint_json: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut actual_output = false;
    while let Some(event) = stream.message().await.unwrap() {
        assert_ne!(event.kind, "error");
        if event.kind == "finished" && event.node_id == loaded.nodes[1].id {
            actual_output = true;
        }
    }
    assert!(actual_output);
    let runs = client
        .list_executions(ListExecutionsRequest {
            workspace_path: path.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(runs.executions[0].status, "Completed");
    let facts: serde_json::Value = serde_json::from_str(&runs.executions[0].data_json).unwrap();
    let invocation = facts["view"]["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["node_id"] == loaded.nodes[1].id)
        .unwrap();
    assert!(invocation["inputs"].as_object().unwrap().values().any(|v| v == &serde_json::json!(7)));
    assert!(
        invocation["outputs"].as_object().unwrap().values().any(|v| v == &serde_json::json!(14))
    );
    dispatch(&mut client, &mut state, parse("uninstall com.test.nodesrpc ws").unwrap())
        .await
        .unwrap();
    assert!(
        client
            .load_blueprint(proto::LoadBlueprintRequest {
                workspace_path: path.clone(),
                blueprint_id: loaded.id
            })
            .await
            .is_err()
    );
    client
        .close_workspace(CloseWorkspaceRequest {
            path,
        })
        .await
        .unwrap();
}
