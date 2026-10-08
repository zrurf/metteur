//! Read-only blueprint libraries; executable effects remain behind owned nodes.
use super::{manifest::Manifest, package::Package};
use crate::{DaemonError, DaemonResult, registry::Registry};
use metteur_shared::{
    FunctionEntry, FunctionSource, NodeType, PinType,
    node_catalog::addon::{BINDING_KEY, FUNCTION_BINDING_KEY},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
fn failure(message: &str) -> DaemonError {
    DaemonError::Addon(message.into())
}
pub(crate) fn value_matches(value: &metteur_shared::Value, ty: &metteur_shared::DataType) -> bool {
    if matches!(ty, metteur_shared::DataType::Any) {
        return true;
    }
    if matches!(ty, metteur_shared::DataType::Context) {
        return matches!(value, metteur_shared::Value::Context(_));
    }
    super::nodes::raw(value)
        .is_ok_and(|v| metteur_shared::node_catalog::addon::json_matches(&v, ty))
}
pub(crate) fn digest(value: &impl serde::Serialize) -> String {
    use sha2::{Digest, Sha256};
    let canonical = serde_json::to_value(value).expect("serializable function contract");
    Sha256::digest(serde_json::to_vec(&canonical).expect("serializable value"))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn stable_id(package: &Package, name: &str, kind: &str, old: uuid::Uuid) -> uuid::Uuid {
    use sha2::{Digest, Sha256};
    let bytes = Sha256::digest(
        format!(
            "{}:{}:{name}:{kind}:{old}",
            package.identity.owner(),
            package.identity.fingerprint
        )
        .as_bytes(),
    );
    let mut id = [0; 16];
    id.copy_from_slice(&bytes[..16]);
    uuid::Uuid::from_bytes(id)
}
fn check_types(value: &serde_json::Value) -> DaemonResult<()> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if (key == "data_type"
                    || (key == "type"
                        && map.get("kind").and_then(|v| v.as_str()).is_some_and(|k| {
                            matches!(k, "data-in" | "data-out" | "exec-in" | "exec-out")
                        })))
                    && let Some(text) = value.as_str()
                {
                    let mut depth = 0i32;
                    if text.len() > 2048 {
                        return Err(failure("Function type limit exceeded"));
                    }
                    for b in text.bytes() {
                        if b == b'<' || b == b'{' {
                            depth += 1;
                        }
                        if b == b'>' || b == b'}' {
                            depth -= 1;
                        }
                        if !(0..=8).contains(&depth) {
                            return Err(failure("Function type nesting limit exceeded"));
                        }
                    }
                    if depth != 0 {
                        return Err(failure("Invalid function type"));
                    }
                }
                check_types(value)?;
            }
        }
        serde_json::Value::Array(a) => {
            for value in a {
                check_types(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn prepare(
    package: &Package,
    decl: &super::manifest::FunctionEntry,
) -> DaemonResult<FunctionEntry> {
    let bytes = super::manifest::package_file(&package.files, &decl.file)?;
    let json: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| failure("Invalid function JSON"))?;
    check_types(&json)?;
    let mut body = crate::storage::blueprint_files::decode(bytes)?;
    if body.nodes.is_empty() || body.nodes.len() > 512 || body.edges.len() > 4096 {
        return Err(failure("Function graph limit exceeded"));
    }
    let prefix = super::pascal(&package.identity.id);
    let name = format!("{prefix}{}", decl.name);
    let id = stable_id(package, &name, "function", uuid::Uuid::nil());
    let mut nodes = BTreeMap::new();
    let mut pins = BTreeMap::new();
    let mut edges = BTreeSet::new();
    for node in &body.nodes {
        if nodes.insert(node.id, stable_id(package, &name, "node", node.id)).is_some() {
            return Err(failure("Duplicate function node identity"));
        }
        for pin in &node.pins {
            if pins.insert(pin.id, stable_id(package, &name, "pin", pin.id)).is_some() {
                return Err(failure("Duplicate function pin identity"));
            }
        }
    }
    let entries: Vec<_> = body.nodes.iter().filter(|n| n.kind == "FunctionEntry").collect();
    if entries.len() != 1
        || entries[0].id != body.entry_node_id
        || body.nodes.iter().filter(|n| n.kind == "FunctionExit").count() != 1
    {
        return Err(failure("Function requires one canonical entry and exit"));
    }
    body.entry_node_id =
        *nodes.get(&body.entry_node_id).ok_or_else(|| failure("Missing function entry"))?;
    body.id = id;
    body.name = name.clone();
    for node in &mut body.nodes {
        node.id = nodes[&node.id];
        for pin in &mut node.pins {
            pin.id = pins[&pin.id];
        }
        if package.manifest.nodes.iter().any(|n| n.name == node.kind) {
            node.kind = format!("{prefix}{}", node.kind);
        }
        if node.kind == "CallFunction"
            && let Some(target) =
                node.data.get("function").and_then(|v| v.as_str()).map(str::to_owned)
            && package.manifest.functions.iter().any(|f| f.name == target)
        {
            node.data["function"] = serde_json::json!(format!("{prefix}{target}"));
        }
    }
    for edge in &mut body.edges {
        if !edges.insert(edge.id) {
            return Err(failure("Duplicate function edge identity"));
        }
        edge.id = stable_id(package, &name, "edge", edge.id);
        edge.source_node =
            *nodes.get(&edge.source_node).ok_or_else(|| failure("Unknown function edge node"))?;
        edge.target_node =
            *nodes.get(&edge.target_node).ok_or_else(|| failure("Unknown function edge node"))?;
        edge.source_pin =
            *pins.get(&edge.source_pin).ok_or_else(|| failure("Unknown function edge pin"))?;
        edge.target_pin =
            *pins.get(&edge.target_pin).ok_or_else(|| failure("Unknown function edge pin"))?;
    }
    let signature = FunctionEntry::derive_signature(&body).map_err(DaemonError::Addon)?;
    let mut names = BTreeSet::new();
    for pin in signature.inputs.iter().chain(&signature.outputs) {
        if pin.name.is_empty()
            || pin.name.len() > 128
            || !pin.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !names.insert(&pin.name)
            || pin.data_type == metteur_shared::DataType::Void
        {
            return Err(failure("Invalid or duplicate function parameter"));
        }
    }
    Ok(FunctionEntry {
        id,
        name,
        description: format!(
            "Read-only addon {}@{}",
            package.identity.id, package.identity.version
        ),
        signature,
        body,
        source: FunctionSource::Addon,
    })
}
fn reference(dependency: &str, package: &Package) -> String {
    if package.manifest.functions.iter().any(|f| f.name == dependency)
        || package.manifest.nodes.iter().any(|n| n.name == dependency)
    {
        format!("{}{}", super::pascal(&package.identity.id), dependency)
    } else {
        dependency.into()
    }
}

pub(super) fn install(registry: &mut Registry, package: &Package) -> DaemonResult<()> {
    let mut pending = BTreeMap::new();
    for decl in &package.manifest.functions {
        let entry = prepare(package, decl)?;
        pending.insert(entry.name.clone(), (entry, decl));
    }
    let mut depth = BTreeMap::<String, usize>::new();
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .find(|(_, (entry, _))| {
                !entry.body.nodes.iter().any(|n| {
                    n.kind == "CallFunction"
                        && n.data
                            .get("function")
                            .and_then(|v| v.as_str())
                            .is_some_and(|name| pending.contains_key(name))
                })
            })
            .map(|(name, _)| name.clone());
        let name = ready.ok_or_else(|| failure("Addon function dependency cycle or recursion"))?;
        let (mut entry, decl) = pending.remove(&name).expect("pending function");
        let declared: BTreeSet<_> =
            decl.dependencies.iter().map(|d| reference(d, package)).collect();
        let mut dependencies = BTreeMap::new();
        let mut level = 1;
        for node in &mut entry.body.nodes {
            let (key, target, binding) = if node.kind == "CallFunction" {
                let target = node
                    .data
                    .get("function")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| failure("Function call target required"))?
                    .to_string();
                let binding =
                    registry.addon_function_bindings.get(&target).cloned().ok_or_else(|| {
                        failure("Function dependency is missing, disabled or not package-owned")
                    })?;
                level = level.max(
                    depth.get(&target).copied().unwrap_or_else(|| {
                        binding.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize
                    }) + 1,
                );
                (FUNCTION_BINDING_KEY, target, binding)
            } else if let Some(binding) =
                registry.node_signatures().addon_bindings.get(&node.kind).cloned()
            {
                (BINDING_KEY, node.kind.clone(), binding)
            } else {
                let signature = metteur_shared::node_catalog::builtin_signature(&node.kind)
                    .ok_or_else(|| failure("Unknown function node dependency"))?;
                if signature.node_type != NodeType::Pure
                    && !matches!(
                        node.kind.as_str(),
                        "FunctionEntry"
                            | "FunctionExit"
                            | "Branch"
                            | "Switch"
                            | "ForEach"
                            | "Delay"
                            | "VariableGet"
                            | "VariableSet"
                    )
                {
                    return Err(failure(
                        "Package functions use owned Wasm nodes for effects; native effect nodes require an explicit editable copy",
                    ));
                }
                continue;
            };
            if !declared.contains(&target) {
                return Err(failure("Function node or function dependency must be declared"));
            }
            if node.data.get(key).is_some_and(|old| old != &binding) {
                return Err(failure("Declared function dependency identity changed"));
            }
            if !node.data.is_object() {
                node.data = serde_json::json!({});
            }
            node.data[key] = binding.clone();
            dependencies.insert(target, digest(&binding));
        }
        if declared.len() != dependencies.len() || level > 32 {
            return Err(failure("Unused/missing function dependency or nesting limit exceeded"));
        }
        let mut catalog = registry.node_signatures();
        catalog.functions.insert(entry.name.clone(), entry.signature.clone());
        let report = metteur_shared::model::validate::validate_with_catalog(&entry.body, &catalog);
        if !report.is_ok() {
            return Err(failure("Invalid addon function graph or pin contract"));
        }
        let mut reached = BTreeSet::from([entry.body.entry_node_id]);
        loop {
            let previous = reached.len();
            for edge in &entry.body.edges {
                if reached.contains(&edge.source_node)
                    && entry
                        .body
                        .pin(edge.source_pin)
                        .is_some_and(|p| p.pin_type == PinType::ExecOutput)
                {
                    reached.insert(edge.target_node);
                }
            }
            if previous == reached.len() {
                break;
            }
        }
        if !entry.body.nodes.iter().any(|n| n.kind == "FunctionExit" && reached.contains(&n.id)) {
            return Err(failure("Function exit is not reachable"));
        }
        let binding = serde_json::json!({"package":package.identity,"name":entry.name,"dependencies":dependencies,"body":digest(&entry.body),"depth":level});
        depth.insert(entry.name.clone(), level);
        registry.register_owned_function(&package.identity.owner(), entry, binding)?;
    }
    Ok(())
}

fn contributed(manifest: &Manifest) -> BTreeSet<String> {
    let prefix = super::pascal(&manifest.id);
    manifest
        .nodes
        .iter()
        .map(|n| format!("{prefix}{}", n.name))
        .chain(manifest.functions.iter().map(|f| format!("{prefix}{}", f.name)))
        .collect()
}
pub(super) fn order(manifests: &[&Manifest]) -> Vec<usize> {
    let names: Vec<_> = manifests.iter().map(|m| contributed(m)).collect();
    let mut pending: BTreeSet<_> = (0..manifests.len()).collect();
    let mut result = vec![];
    while !pending.is_empty() {
        let next = pending.iter().copied().find(|i| {
            !manifests[*i]
                .functions
                .iter()
                .flat_map(|f| &f.dependencies)
                .any(|dep| pending.iter().any(|j| i != j && names[*j].contains(dep)))
        });
        let Some(next) = next else {
            result.extend(pending);
            break;
        };
        pending.remove(&next);
        result.push(next);
    }
    result
}
pub(super) fn order_dirs(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let manifests: Vec<_> = dirs.iter().map(|p| Manifest::load(p).ok().map(|(m, _)| m)).collect();
    let valid: Vec<_> =
        manifests.iter().enumerate().filter_map(|(i, m)| m.as_ref().map(|m| (i, m))).collect();
    let refs: Vec<_> = valid.iter().map(|(_, m)| *m).collect();
    order(&refs)
        .into_iter()
        .map(|i| dirs[valid[i].0].clone())
        .chain(
            dirs.iter()
                .enumerate()
                .filter(|(i, _)| manifests[*i].is_none())
                .map(|(_, p)| p.clone()),
        )
        .collect()
}

#[cfg(test)]
#[path = "functions_tests.rs"]
mod tests;
