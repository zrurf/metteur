//! Authoritative blueprint files; database records are only mirrors and bindings.
use metteur_shared::Blueprint;
use serde_json::{Value, json};
use uuid::Uuid;

use super::persistence::{Db, cf};
use super::versioning::{VersionManager, VersionRef};
use crate::error::{DaemonError, DaemonResult};

fn binding_key(id: Uuid) -> Vec<u8> {
    format!("blueprint-file:{id}").into_bytes()
}

pub fn binding(db: &Db, id: Uuid) -> DaemonResult<Option<VersionRef>> {
    db.get(cf::BLUEPRINTS, &binding_key(id))?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|e| DaemonError::Serialization(e.to_string()))
        })
        .transpose()
}

pub fn bindings(db: &Db) -> DaemonResult<Vec<(Uuid, VersionRef)>> {
    db.scan(cf::BLUEPRINTS)?
        .into_iter()
        .filter(|(k, _)| k.starts_with(b"blueprint-file:"))
        .map(|(k, v)| {
            let id = std::str::from_utf8(&k[b"blueprint-file:".len()..])
                .ok()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| DaemonError::Persistence("invalid blueprint file binding".into()))?;
            let version = serde_json::from_slice(&v)
                .map_err(|e| DaemonError::Serialization(e.to_string()))?;
            Ok((id, version))
        })
        .collect()
}

/// Manual load/new execution reads the file. A watcher never calls this or
/// changes a running SharedBlueprint. Old database-only graphs remain readable.
pub fn load(db: &Db, versions: &VersionManager, id: Uuid) -> DaemonResult<Blueprint> {
    let _guard = versions.blueprint_gate.lock();
    let Some(bound) = binding(db, id)? else {
        return decode(
            &db.get(cf::BLUEPRINTS, id.as_bytes())?
                .ok_or_else(|| DaemonError::NotFound("blueprint not found".into()))?,
        );
    };
    crate::replan::application::ensure_resolved(db)?;
    let bytes = std::fs::read(versions.blueprint_path(&bound.blueprint_uri)?)?;
    let graph = decode(&bytes)?;
    if graph.id != id {
        return Err(DaemonError::Execution(
            "authoritative file now contains a different blueprint id".into(),
        ));
    }
    let version = versions.capture_blueprint(&bound.blueprint_uri)?;
    if crate::storage::versioning::hash_content(&bytes) != version.blob_hash {
        return Err(DaemonError::Execution("blueprint changed while loading; retry".into()));
    }
    db.put_pair(
        cf::BLUEPRINTS,
        id.as_bytes(),
        &encode_native(&graph)?,
        &binding_key(id),
        &serde_json::to_vec(&version).map_err(|e| DaemonError::Serialization(e.to_string()))?,
    )?;
    Ok(graph)
}

/// Decode both existing native/CLI JSON and the editor's canvas representation.
pub fn decode(bytes: &[u8]) -> DaemonResult<Blueprint> {
    let mut doc: Value =
        serde_json::from_slice(bytes).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    let mut null_defaults: Vec<String> = doc["explicit_null_defaults"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if doc.get("entry_node_id").is_none() {
        let mut nodes = Vec::new();
        for node in doc["nodes"]
            .as_array()
            .ok_or_else(|| DaemonError::Execution("blueprint nodes are missing".into()))?
        {
            if node["type"] == "FileReference" {
                continue;
            }
            let mut data = node.get("data").cloned().unwrap_or(json!({}));
            let mut pins = Vec::new();
            for field in ["inputs", "outputs"] {
                for pin in node[field].as_array().into_iter().flatten() {
                    let kind = pin["kind"].as_str().unwrap_or("");
                    let id = pin["id"].as_str().unwrap_or("");
                    if pin.get("default") == Some(&Value::Null) {
                        null_defaults.push(id.to_owned());
                    }
                    let key = pin["key"].as_str().unwrap_or("");
                    let name = pin["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(key);
                    let ty = pin["type"].as_str().unwrap_or(if kind.starts_with("exec") {
                        "void"
                    } else {
                        "any"
                    });
                    let raw = node["values"]
                        .get(id)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| {
                            [name, key, id]
                                .into_iter()
                                .filter(|key| !key.is_empty())
                                .find_map(|key| data.get(key))
                                .map(|v| {
                                    v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())
                                })
                        });
                    if kind == "data-in"
                        && let Some(raw) = raw.as_deref()
                    {
                        let object = data.as_object_mut().ok_or_else(|| {
                            DaemonError::Execution("node data must be an object".into())
                        })?;
                        object.remove(name);
                        object.remove(key);
                        object.remove(id);
                        if !raw.is_empty() {
                            let value = match ty {
                                "int" | "float" | "number" => raw
                                    .parse::<f64>()
                                    .ok()
                                    .filter(|v| v.is_finite())
                                    .map(|v| {
                                        if v.fract() == 0.0
                                            && v >= i64::MIN as f64
                                            && v < i64::MAX as f64
                                        {
                                            json!(v as i64)
                                        } else {
                                            json!(v)
                                        }
                                    })
                                    .unwrap_or(json!(raw)),
                                "bool" => json!(raw == "true"),
                                _ if ty == "any"
                                    || ty == "json"
                                    || ty.starts_with("list")
                                    || ty.starts_with("object") =>
                                {
                                    serde_json::from_str(raw).unwrap_or(json!(raw))
                                }
                                _ => json!(raw),
                            };
                            object.insert(name.into(), value);
                        }
                    }
                    pins.push(json!({"id":id,"key": if key.is_empty() { None } else { Some(key) },"name":name,
                        "pin_type":match kind { "exec-in"=>"ExecInput","exec-out"=>"ExecOutput","data-out"=>"DataOutput",_=>"DataInput" },
                        "data_type":ty,"default":pin.get("default"),"optional":pin["optional"].as_bool().unwrap_or(false),
                        "choices":pin.get("choices").cloned().unwrap_or(json!([])),"description":pin["description"].as_str().filter(|v| !v.is_empty())}));
                }
            }
            nodes.push(json!({"id":node["id"],"kind":if node["type"] == "Arithmetic" { json!("Add") } else {node["type"].clone()},
                "node_type":node["nodeType"],"position":[node["position"]["x"],node["position"]["y"]],"pins":pins,"data":data}));
        }
        let ids: Vec<_> = nodes.iter().filter_map(|n| n["id"].as_str()).collect();
        let entry = doc["entryNodeId"]
            .as_str()
            .filter(|id| ids.contains(id))
            .or_else(|| ids.first().copied())
            .unwrap_or("");
        let edges: Vec<_> = doc["edges"].as_array().into_iter().flatten().filter(|e| ids.contains(&e["source"].as_str().unwrap_or("")) && ids.contains(&e["target"].as_str().unwrap_or(""))).map(|e| json!({"id":e["id"],"source_node":e["source"],"source_pin":e["sourceHandle"],"target_node":e["target"],"target_pin":e["targetHandle"]})).collect();
        doc = json!({"id":doc["id"],"name":doc["name"],"entry_node_id":entry,"nodes":nodes,"edges":edges});
    }
    let mut graph: Blueprint =
        serde_json::from_value(doc).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    for pin in graph.nodes.iter_mut().flat_map(|n| &mut n.pins) {
        if null_defaults.contains(&pin.id.to_string()) {
            pin.default = Some(Value::Null);
        }
    }
    Ok(graph)
}

pub fn save(
    db: &Db,
    versions: &VersionManager,
    blueprint: &Blueprint,
    uri: &str,
    bytes: &[u8],
    expected: Option<&VersionRef>,
) -> DaemonResult<VersionRef> {
    let expected = expected.map(super::versioning::ExpectedFile::Version)
        .unwrap_or(super::versioning::ExpectedFile::Blueprint(blueprint.id));
    save_with_origin(db, versions, blueprint, uri, bytes, expected, None)
        .map(|(version, _)| version)
}

pub(crate) fn save_with_origin(
    db: &Db,
    versions: &VersionManager,
    blueprint: &Blueprint,
    uri: &str,
    bytes: &[u8],
    expected: super::versioning::ExpectedFile<'_>,
    origin: Option<crate::execution::file_journal::FileOrigin>,
) -> DaemonResult<(VersionRef, Option<Uuid>)> {
    let _guard = versions.blueprint_gate.lock();
    if decode(bytes)? != *blueprint {
        return Err(DaemonError::Execution("blueprint file and executable graph disagree".into()));
    }
    let path = versions.blueprint_path(uri)?;
    if let Some(current) = binding(db, blueprint.id)?
        && versions.blueprint_path(&current.blueprint_uri)? != path
    {
        return Err(DaemonError::Execution(
            "blueprint already belongs to another file; use a new id for a copy".into(),
        ));
    }
    for (key, value) in db.scan(cf::BLUEPRINTS)? {
        if key.starts_with(b"blueprint-file:") && key != binding_key(blueprint.id) {
            let other: VersionRef = serde_json::from_slice(&value)
                .map_err(|e| DaemonError::Serialization(e.to_string()))?;
            if versions.blueprint_path(&other.blueprint_uri)? == path {
                return Err(DaemonError::Execution(
                    "file already belongs to another blueprint".into(),
                ));
            }
        }
    }
    let (version, operation_id) = versions.write_blueprint_with_origin(
        uri,
        bytes,
        expected,
        origin.unwrap_or(crate::execution::file_journal::FileOrigin {
            run_id: Uuid::nil(),
            node_id: Uuid::nil(),
            attempt: 0,
            wal_position: 0,
        }),
    )?;
    let graph = encode_native(blueprint)?;
    let metadata =
        serde_json::to_vec(&version).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    db.put_pair(
        cf::BLUEPRINTS,
        blueprint.id.as_bytes(),
        &graph,
        &binding_key(blueprint.id),
        &metadata,
    )?;
    Ok((version, operation_id))
}

/// Preserve canvas-only fields while applying parameter edits to its executable graph.
pub fn encode_changes(original: &[u8], graph: &Blueprint) -> DaemonResult<Vec<u8>> {
    let mut doc: Value =
        serde_json::from_slice(original).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    if doc.get("entry_node_id").is_some() {
        return encode_native(graph);
    }
    for node in doc["nodes"]
        .as_array_mut()
        .ok_or_else(|| DaemonError::Execution("missing canvas nodes".into()))?
    {
        let Some(graph_node) =
            graph.nodes.iter().find(|n| Some(n.id.to_string()).as_deref() == node["id"].as_str())
        else {
            continue;
        };
        node["data"] = graph_node.data.clone();
        let mut values = serde_json::Map::new();
        for pin in &graph_node.pins {
            if pin.pin_type == metteur_shared::PinType::DataInput {
                let raw = graph_node
                    .data
                    .get(&pin.name)
                    .map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))
                    .unwrap_or_default();
                values.insert(pin.id.to_string(), Value::String(raw));
            }
        }
        node["values"] = Value::Object(values);
    }
    let bytes =
        serde_json::to_vec_pretty(&doc).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    if decode(&bytes)? != *graph {
        return Err(DaemonError::Execution(
            "canvas changes do not round-trip; save the file explicitly".into(),
        ));
    }
    Ok(bytes)
}

/// Native JSON historically encodes both absent defaults and explicit null as
/// null. Preserve that legacy meaning and annotate only explicit null defaults.
pub fn encode_native(graph: &Blueprint) -> DaemonResult<Vec<u8>> {
    let mut doc =
        serde_json::to_value(graph).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    let nulls: Vec<_> = graph
        .nodes
        .iter()
        .flat_map(|n| &n.pins)
        .filter(|p| p.default == Some(Value::Null))
        .map(|p| p.id.to_string())
        .collect();
    if !nulls.is_empty() {
        doc["explicit_null_defaults"] = json!(nulls);
    }
    serde_json::to_vec_pretty(&doc).map_err(|e| DaemonError::Serialization(e.to_string()))
}
