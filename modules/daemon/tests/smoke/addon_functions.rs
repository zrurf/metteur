use super::*;
use metteur_shared::{Blueprint, DataType, Edge, Node, NodeType, Pin, PinType};
pub(super) fn body(kind: &str) -> Blueprint {
    let signature = metteur_shared::node_catalog::builtin_signature(kind).unwrap();
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

#[tokio::test]
async fn readonly_function_cli_import_uses_existing_file_versions_and_preserves_source() {
    use metteur_cli::commands::{SessionState, dispatch, parse};
    let (mut client, root) = start_server(Default::default()).await;
    let path = root.to_string_lossy().into_owned();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: path.clone(),
        })
        .await
        .unwrap();
    let source = root.join("r12-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("manifest.toml"),"id=\"com.test.library\"\nversion=\"1.0.0\"\nname=\"Function library\"\n[[functions]]\nname=\"Library\"\nfile=\"library.blueprint\"\n").unwrap();
    let original = metteur_daemon::storage::blueprint_files::encode_native(&body("Add")).unwrap();
    std::fs::write(source.join("library.blueprint"), &original).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[31; 32]);
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
    let name = "ComTestLibraryLibrary";
    let loaded = client
        .load_function(LoadFunctionRequest {
            workspace_path: path.clone(),
            name: name.into(),
        })
        .await
        .unwrap()
        .into_inner();
    let info = loaded.info.unwrap();
    assert_eq!(info.source, "addon");
    assert!(!info.addon_binding_json.is_empty());
    assert_eq!(info.inputs[0].name, "A");
    assert_eq!(info.outputs[0].name, "Result");
    assert!(
        client
            .delete_function(DeleteFunctionRequest {
                workspace_path: path.clone(),
                name: name.into()
            })
            .await
            .is_err()
    );
    assert!(
        client
            .save_function(SaveFunctionRequest {
                workspace_path: path.clone(),
                info: Some(info.clone()),
                body: loaded.body.clone(),
                ..Default::default()
            })
            .await
            .is_err()
    );
    let import = |copy: &str, file: &str, binding: &str| SaveFunctionRequest {
        workspace_path: path.clone(),
        info: Some(FunctionInfo {
            name: copy.into(),
            ..Default::default()
        }),
        import_from: name.into(),
        file_path: file.into(),
        expected_addon_binding_json: binding.into(),
        ..Default::default()
    };
    assert!(client.save_function(import("StaleCopy", "stale.blueprint", "{}")).await.is_err());
    assert!(!root.join("stale.blueprint").exists());
    std::fs::write(root.join("occupied.blueprint"), "preserve").unwrap();
    assert!(
        client
            .save_function(import("OccupiedCopy", "occupied.blueprint", &info.addon_binding_json))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(root.join("occupied.blueprint")).unwrap(), "preserve");
    assert!(
        client
            .save_function(import("Add", "conflict.blueprint", &info.addon_binding_json))
            .await
            .is_err()
    );
    dispatch(
        &mut client,
        &mut state,
        parse(&format!("func import {name} EditableLibrary functions/editable.blueprint")).unwrap(),
    )
    .await
    .unwrap();
    let copy = client
        .load_function(LoadFunctionRequest {
            workspace_path: path.clone(),
            name: "EditableLibrary".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let copy_info = copy.info.unwrap();
    assert_eq!(copy_info.source, "workspace");
    assert_ne!(copy_info.id, info.id);
    assert!(copy_info.file_path.ends_with("functions/editable.blueprint"));
    assert!(
        client
            .save_function(import("EditableLibrary", "other.blueprint", &info.addon_binding_json))
            .await
            .is_err()
    );
    let history = client
        .get_file_history(GetFileHistoryRequest {
            workspace_path: path.clone(),
            path: copy_info.file_path.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!history.entries.is_empty());
    let mut edited = copy.body.unwrap();
    let compute = edited.nodes.iter_mut().find(|n| n.kind == "Add").unwrap();
    compute.data_json = serde_json::json!({"b":8}).to_string();
    client
        .save_function(SaveFunctionRequest {
            workspace_path: path.clone(),
            info: Some(copy_info.clone()),
            body: Some(edited),
            ..Default::default()
        })
        .await
        .unwrap();
    let newer = client
        .get_file_history(GetFileHistoryRequest {
            workspace_path: path.clone(),
            path: copy_info.file_path.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(newer.entries.len() > history.entries.len());
    assert_eq!(std::fs::read(source.join("library.blueprint")).unwrap(), original);
    for (function, expected) in [(name, 14), ("EditableLibrary", 15)] {
        let graph=client.compile_dsl(CompileDslRequest{workspace_path:path.clone(),source:format!("entry s: Start\nf: CallFunction(function = \"{function}\", A = 7)\ne: End\ns -> f\nf -> e\n")}).await.unwrap().into_inner();
        client
            .save_blueprint(SaveBlueprintRequest {
                workspace_path: path.clone(),
                blueprint: Some(graph.clone()),
                file_path: format!("{function}.blueprint"),
                file_json: String::new(),
            })
            .await
            .unwrap();
        let mut stream = client
            .execute_blueprint(ExecuteBlueprintRequest {
                workspace_path: path.clone(),
                blueprint_id: graph.id,
                blueprint_json: String::new(),
            })
            .await
            .unwrap()
            .into_inner();
        while let Some(event) = stream.message().await.unwrap() {
            assert_ne!(event.kind, "error", "{}", event.message);
        }
        let runs = client
            .list_executions(ListExecutionsRequest {
                workspace_path: path.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        let facts: serde_json::Value = serde_json::from_str(
            &runs
                .executions
                .iter()
                .find(|r| {
                    r.status == "Completed"
                        && serde_json::from_str::<serde_json::Value>(&r.data_json).unwrap()["view"]
                            ["invocations"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|i| i["node_id"] == graph.nodes[1].id)
                })
                .unwrap()
                .data_json,
        )
        .unwrap();
        assert!(facts["view"]["invocations"].as_array().unwrap().iter().any(|i| {
            i["node_id"] == graph.nodes[1].id
                && i["outputs"]
                    .as_object()
                    .is_some_and(|o| o.values().any(|v| v.as_f64() == Some(f64::from(expected))))
        }));
    }
    let other = root.join("other");
    std::fs::create_dir(&other).unwrap();
    let other = other.to_string_lossy().into_owned();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: other.clone(),
        })
        .await
        .unwrap();
    assert!(
        client
            .load_function(LoadFunctionRequest {
                workspace_path: other.clone(),
                name: "EditableLibrary".into()
            })
            .await
            .is_err()
    );
    dispatch(&mut client, &mut state, parse("uninstall com.test.library ws").unwrap())
        .await
        .unwrap();
    assert!(
        client
            .load_function(LoadFunctionRequest {
                workspace_path: path.clone(),
                name: name.into()
            })
            .await
            .is_err()
    );
    assert!(
        client
            .load_function(LoadFunctionRequest {
                workspace_path: path.clone(),
                name: "EditableLibrary".into()
            })
            .await
            .is_ok()
    );
    client
        .close_workspace(CloseWorkspaceRequest {
            path: other,
        })
        .await
        .unwrap();
    client
        .close_workspace(CloseWorkspaceRequest {
            path,
        })
        .await
        .unwrap();
}
