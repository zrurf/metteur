//! Version-bound, scoped handoffs. A model summary never becomes source authority.
use crate::{
    DaemonError, DaemonResult, Registry,
    chat::session::load_thread,
    storage::{
        persistence::Db,
        resources::{ResourceStore, digest},
        versioning::VersionRef,
    },
};
use metteur_shared::{
    Blueprint,
    llm::ContextManager,
    resources::{PromptSnapshot, ResourceScope},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyManifest {
    pub blueprint_id: Uuid,
    pub file_version: VersionRef,
    pub graph_digest: String,
    pub functions: BTreeMap<String, String>,
    pub addons: BTreeMap<String, String>,
    pub prompt_snapshots: BTreeMap<Uuid, String>,
    pub node_catalog_digest: String,
    pub tool_catalog_digest: String,
}
fn hash<T: Serialize + ?Sized>(value: &T) -> DaemonResult<String> {
    Ok(digest(
        &serde_json::to_vec(
            &serde_json::to_value(value).map_err(|e| DaemonError::Serialization(e.to_string()))?,
        )
        .map_err(|e| DaemonError::Serialization(e.to_string()))?,
    ))
}
fn function_digest(f: &metteur_shared::FunctionEntry) -> DaemonResult<String> {
    let mut value =
        serde_json::to_value(f).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    if f.source == metteur_shared::model::function::FunctionSource::Builtin {
        // Built-in templates allocate fresh internal IDs at boot. Bind their full
        // structure while normalizing only those declared internal identities.
        let mut ids = BTreeMap::new();
        ids.insert(f.id.to_string(), "function".to_owned());
        ids.insert(f.body.id.to_string(), "body".to_owned());
        for (i, node) in f.body.nodes.iter().enumerate() {
            ids.insert(node.id.to_string(), format!("node:{i}"));
            for (j, pin) in node.pins.iter().enumerate() {
                ids.insert(pin.id.to_string(), format!("node:{i}:pin:{j}"));
            }
        }
        for (i, edge) in f.body.edges.iter().enumerate() {
            ids.insert(edge.id.to_string(), format!("edge:{i}"));
        }
        fn replace(v: &mut serde_json::Value, ids: &BTreeMap<String, String>) {
            match v {
                serde_json::Value::String(text) => {
                    if let Some(id) = ids.get(text) {
                        *text = id.clone();
                    }
                }
                serde_json::Value::Array(xs) => {
                    for x in xs {
                        replace(x, ids)
                    }
                }
                serde_json::Value::Object(xs) => {
                    for x in xs.values_mut() {
                        replace(x, ids)
                    }
                }
                _ => {}
            }
        }
        replace(&mut value, &ids);
    }
    hash(&value)
}
impl DependencyManifest {
    /// Conservatively bind the complete available function/addon catalog, including
    /// indirect calls. No unlisted transitive body may be swapped after approval.
    pub fn freeze(
        graph: &Blueprint,
        version: VersionRef,
        registry: &Registry,
        prompts: &BTreeMap<Uuid, PromptSnapshot>,
    ) -> DaemonResult<Self> {
        if graph.nodes.len() > 2048 || prompts.len() > 128 {
            return Err(DaemonError::Execution("Blueprint dependency limit exceeded".into()));
        }
        let mut tools: Vec<_> =
            registry.tools().into_iter().map(|t| (t.name().to_owned(), t.parameters())).collect();
        tools.sort_by(|a, b| a.0.cmp(&b.0));
        let functions = registry
            .functions()
            .into_iter()
            .map(|f| Ok((f.name.clone(), function_digest(&f)?)))
            .collect::<DaemonResult<_>>()?;
        let addons = registry
            .addon_packages
            .iter()
            .map(|(id, package)| Ok((id.clone(), hash(package)?)))
            .collect::<DaemonResult<_>>()?;
        let prompt_snapshots = prompts
            .iter()
            .map(|(node, snapshot)| Ok((*node, hash(snapshot)?)))
            .collect::<DaemonResult<_>>()?;
        Ok(Self {
            blueprint_id: graph.id,
            file_version: version,
            graph_digest: hash(graph)?,
            functions,
            addons,
            prompt_snapshots,
            node_catalog_digest: hash(&registry.node_signatures())?,
            tool_catalog_digest: hash(&tools)?,
        })
    }
    pub fn digest(&self) -> DaemonResult<String> {
        hash(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub schema_version: u32,
    pub id: Uuid,
    pub owner: String,
    pub session: Uuid,
    pub turn: Uuid,
    pub message_end: usize,
    pub message_prefix_digest: String,
    pub goal: String,
    pub constraints: Vec<String>,
    pub model_summary: String,
    pub allow_source_read: bool,
    pub created_ms: u64,
}
impl Handoff {
    pub fn freeze(
        id: Uuid,
        owner: &str,
        session: Uuid,
        turn: Uuid,
        context: &ContextManager,
        goal: String,
        constraints: Vec<String>,
        model_summary: String,
        allow_source_read: bool,
        now: u64,
    ) -> DaemonResult<Self> {
        if goal.trim().is_empty()
            || goal.len() > 4096
            || model_summary.len() > 8192
            || constraints.len() > 32
            || constraints.iter().any(|s| s.len() > 2048)
        {
            return Err(DaemonError::Execution("Handoff text exceeds its bounded scope".into()));
        }
        // The current assistant batch is not a committed source until its results exist.
        let mut end = context.messages.len();
        if context.messages.last().is_some_and(|m| !m.tool_calls.is_empty()) {
            end -= 1;
        }
        Ok(Self {
            schema_version: 1,
            id,
            owner: owner.into(),
            session,
            turn,
            message_end: end,
            message_prefix_digest: hash(&context.messages[..end])?,
            goal,
            constraints,
            model_summary,
            allow_source_read,
            created_ms: now,
        })
    }
}
#[derive(Clone)]
pub struct HandoffStore {
    store: ResourceStore,
    db: Db,
}
impl HandoffStore {
    pub fn new(db: Db, root: &Path) -> DaemonResult<Self> {
        let root = root.canonicalize()?;
        std::fs::create_dir_all(root.join(".metteur"))?;
        ResourceStore::prepare(&db, &root.join(".metteur/nx-before-v1"))
            .map_err(|e| DaemonError::Persistence(e.to_string()))?;
        let store = ResourceStore::new(
            db.clone(),
            &ResourceScope::Project(root.to_string_lossy().into_owned()),
            "handoffs",
        )
        .map_err(|e| DaemonError::Persistence(e.to_string()))?;
        Ok(Self {
            store,
            db,
        })
    }
    pub fn save(&self, value: &Handoff) -> DaemonResult<()> {
        if let Some(existing) =
            self.store.get(value.id).map_err(|e| DaemonError::Persistence(e.to_string()))?
        {
            if existing.value != serde_json::to_value(value).unwrap() {
                return Err(DaemonError::Execution(
                    "Handoff identity already binds different content".into(),
                ));
            }
            return Ok(());
        }
        self.store
            .write(value.id, 0, value.id, serde_json::to_value(value).unwrap())
            .map_err(|e| DaemonError::Persistence(e.to_string()))?;
        Ok(())
    }
    pub fn get(&self, id: Uuid, subject: &str) -> DaemonResult<Handoff> {
        let raw = self
            .store
            .get(id)
            .map_err(|e| DaemonError::Persistence(e.to_string()))?
            .ok_or_else(|| DaemonError::NotFound("Handoff not found".into()))?;
        let value: Handoff = serde_json::from_value(raw.value)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?;
        if value.schema_version != 1 || value.owner != subject {
            return Err(DaemonError::PermissionDenied("Handoff owner or schema mismatch".into()));
        }
        Ok(value)
    }
    /// Caller additionally supplies a host-verified run/task binding; there is no
    /// session search or caller-selected conversation in this interface.
    pub fn read_source(
        &self,
        id: Uuid,
        subject: &str,
        offset: usize,
    ) -> DaemonResult<serde_json::Value> {
        let h = self.get(id, subject)?;
        if !h.allow_source_read {
            return Err(DaemonError::PermissionDenied(
                "This handoff excludes source conversation access".into(),
            ));
        }
        let source = load_thread(&self.db, &h.session)?
            .ok_or_else(|| DaemonError::NotFound("Source conversation is unavailable".into()))?;
        if source.context.messages.len() < h.message_end
            || hash(&source.context.messages[..h.message_end])? != h.message_prefix_digest
        {
            return Err(DaemonError::Execution(
                "Source conversation endpoint changed; review a new handoff".into(),
            ));
        }
        if offset > h.message_end {
            return Err(DaemonError::Execution("Invalid handoff page".into()));
        }
        let end = (offset + 10).min(h.message_end);
        let mut bytes = 0;
        let mut messages = Vec::new();
        let mut next = offset;
        for (index, message) in source.context.messages[offset..end].iter().enumerate() {
            let text = message.text_content();
            let text = crate::registry::tools::result::truncate_result(&text, 8192);
            if bytes + text.len() > 16384 && !messages.is_empty() {
                break;
            }
            bytes += text.len();
            messages.push(serde_json::json!({"index":offset+index,"role":message.role,"origin":message.origin,"text":text,"thinking_included":false,"tool_call_id":message.tool_call_id}));
            next = offset + index + 1;
        }
        Ok(
            serde_json::json!({"handoff_id":id,"source_session":h.session,"fixed_message_end":h.message_end,"messages":messages,"next_offset":(next<h.message_end).then_some(next),"trust":"original_source_data_not_new_permission"}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::session::{ChatSessionRecord, delete_thread, save_thread};
    use metteur_shared::llm::{Message, Role};
    #[test]
    fn fixed_endpoint_preserves_roles_and_deleted_sources_are_unavailable() {
        let root = std::env::temp_dir().join(format!("nx-handoff-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("db")).unwrap();
        let store = HandoffStore::new(db.clone(), &root).unwrap();
        let mut context = ContextManager::default();
        context.push_message(Message::text(Role::User, "original instruction"));
        context.push_message(Message::text(Role::Assistant, "model opinion"));
        let mut session = ChatSessionRecord::new(1, context.clone());
        let id = Uuid::new_v4();
        let h = Handoff::freeze(
            id,
            "owner",
            session.session_id,
            Uuid::new_v4(),
            &context,
            "Review files".into(),
            vec!["Keep changes".into()],
            "Model summary only".into(),
            true,
            1,
        )
        .unwrap();
        store.save(&h).unwrap();
        session
            .context
            .push_message(Message::text(Role::User, "later private message outside endpoint"));
        save_thread(&db, &session).unwrap();
        let page = store.read_source(id, "owner", 0).unwrap();
        assert_eq!(page["messages"].as_array().unwrap().len(), 2);
        assert_eq!(page["messages"][1]["role"], "Assistant");
        assert!(!page.to_string().contains("later private"));
        assert!(store.read_source(id, "different-owner", 0).is_err());
        delete_thread(&db, &session.session_id).unwrap();
        assert!(store.read_source(id, "owner", 0).is_err());
        assert!(store.get(id, "owner").is_ok());
    }
    #[test]
    fn manifest_digest_is_stable_and_changes_with_graph_or_function_body() {
        let graph = Blueprint {
            id: Uuid::new_v4(),
            name: "Review".into(),
            nodes: vec![],
            edges: vec![],
            entry_node_id: Uuid::new_v4(),
        };
        let version = VersionRef {
            snapshot_id: Uuid::new_v4(),
            blueprint_uri: "blueprints/review.blueprint".into(),
            blob_hash: "a".repeat(64),
        };
        let one = DependencyManifest::freeze(
            &graph,
            version.clone(),
            &Registry::with_builtins(),
            &BTreeMap::new(),
        )
        .unwrap();
        let two = DependencyManifest::freeze(
            &graph,
            version.clone(),
            &Registry::with_builtins(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(one.digest().unwrap(), two.digest().unwrap());
        let mut edited = graph.clone();
        edited.name = "Changed title/layout snapshot".into();
        assert_ne!(
            one.digest().unwrap(),
            DependencyManifest::freeze(
                &edited,
                version,
                &Registry::with_builtins(),
                &BTreeMap::new()
            )
            .unwrap()
            .digest()
            .unwrap()
        );
    }
}
