//! Resource registry for tools, functions and node executors.

pub mod library;
pub mod node;
pub mod tool;
pub mod tools;

use std::collections::HashMap;
use std::sync::Arc;

use metteur_shared::model::function::{FunctionEntry, FunctionSource};
use parking_lot::RwLock;

use crate::error::{DaemonError, DaemonResult};

pub use node::{NodeExecutor, NodeRegistry};
pub use tool::Tool;

/// The central registry of daemon resources.
///
/// Tools and functions live behind internal locks so hosts such as the MCP
/// manager, the addon system and workspace open/close can register or retire
/// them at runtime.
#[derive(Default)]
pub struct Registry {
    tools: RwLock<Tools>,
    nodes: Arc<NodeRegistry>,
    functions: RwLock<HashMap<String, FunctionEntry>>,
    pub(crate) addon_function_bindings: std::collections::BTreeMap<String, serde_json::Value>,
    function_owners: std::collections::BTreeMap<String,String>,
    pub(crate) addon_packages: std::collections::BTreeMap<String, crate::addon::package::Identity>,
    pub(crate) addon_fragments:
        std::collections::BTreeMap<String, Vec<metteur_shared::SystemFragment>>,
    pub(crate) addon_lease: Option<Arc<crate::addon::services::Lease>>,
    pub(crate) addon_lsp: Vec<Arc<crate::integration::lsp::LspManager>>,
    pub(crate) addon_lsp_claims: std::collections::BTreeMap<String, String>,
    pub(crate) addon_hooks: Vec<Arc<crate::addon::hooks::Binding>>,
}

#[derive(Default, Clone)]
struct Tools {
    values: HashMap<String, Arc<dyn Tool>>,
    owners: HashMap<String, String>,
}

impl Registry {
    /// Creates a registry pre-populated with built-in resources.
    pub fn with_builtins() -> Self {
        let registry = Self {
            nodes: Arc::new(NodeRegistry::with_builtins()),
            ..Default::default()
        };
        for tool in [
            Arc::new(tools::fs_tools::ReadFile) as Arc<dyn Tool>,
            Arc::new(tools::fs_tools::WriteFile),
            Arc::new(tools::edit::EditFile),
            Arc::new(tools::fs_tools::ListDirectory),
            Arc::new(tools::fs_tools::SearchFile),
            Arc::new(tools::search::Grep),
            Arc::new(tools::search::Glob),
            Arc::new(tools::command::ExecuteCommand),
            Arc::new(tools::command::StartCommand),
            Arc::new(tools::command::JobStatus),
            Arc::new(tools::command::WaitJob),
            Arc::new(tools::command::KillJob),
            Arc::new(tools::deps::GetDependencies),
            Arc::new(tools::subagent::SpawnSubAgent),
            Arc::new(tools::lsp_tools::CheckDiagnostics),
            Arc::new(tools::lsp_tools::GetHover),
            Arc::new(tools::lsp_tools::FindDefinition),
            Arc::new(tools::replan::ReplanBlueprint),
            Arc::new(tools::snapshot::SnapshotTake),
            Arc::new(tools::blueprint::DraftBlueprint),
            Arc::new(tools::todo::TodoWrite),
            Arc::new(tools::todo::TodoRead),
            Arc::new(tools::context::ReleaseContext),
        ] {
            registry.try_register_tool(tool).expect("built-in tool names are valid");
        }
        registry.register_function(library::builtin_chain_of_thought());
        registry
    }

    /// Registers a tool, replacing any existing one with the same name.
    ///
    /// Fails when the name is not PascalCase; generated MCP names use
    /// conflict suffixes (`GitStatus_2`) that stay valid under this rule.
    pub fn try_register_tool(&self, tool: Arc<dyn Tool>) -> DaemonResult<()> {
        let name = tool.name().to_string();
        if !tool::is_valid_tool_name(&name) {
            return Err(DaemonError::Execution(format!(
                "tool name '{name}' must be PascalCase imperative"
            )));
        }
        let mut tools = self.tools.write();
        if tools.owners.contains_key(&name) {
            return Err(DaemonError::Addon(format!("tool '{name}' is owned by an addon")));
        }
        tools.values.insert(name, tool);
        Ok(())
    }

    /// Registers a tool, replacing any existing one with the same name.
    ///
    /// Panics when the name is not PascalCase; prefer
    /// [`Self::try_register_tool`] in fallible paths.
    pub fn register_tool(&self, tool: Arc<dyn Tool>) {
        self.try_register_tool(tool).expect("valid tool name");
    }

    /// Removes a tool by name, returning it when it was registered.
    pub fn unregister_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let mut tools = self.tools.write();
        if tools.owners.contains_key(name) {
            return None;
        }
        tools.values.remove(name)
    }

    /// Returns a tool by name, if registered.
    pub fn tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().values.get(name).cloned()
    }

    /// Returns all registered tools, sorted by name.
    ///
    /// Sorted output is what makes the request body stable across calls: an
    /// unordered iteration would reshuffle the tool array and defeat the
    /// provider's prefix cache.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = self.tools.read().values.values().cloned().collect();
        tools.sort_by(|a, b| a.name().cmp(b.name()));
        tools
    }

    /// Returns all registered tool names, sorted.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.read().values.keys().cloned().collect();
        names.sort();
        names
    }

    /// Copy a stable registry for a workspace or execution. Later registrations
    /// in another workspace cannot silently change the captured tool handles.
    pub(crate) fn snapshot(&self) -> Self {
        Self {
            tools: RwLock::new(self.tools.read().clone()),
            nodes: self.nodes.clone(),
            functions: RwLock::new(self.functions.read().clone()),
            addon_function_bindings:self.addon_function_bindings.clone(),
            function_owners:self.function_owners.clone(),
            addon_packages: self.addon_packages.clone(),
            addon_fragments: self.addon_fragments.clone(),
            addon_lease: self.addon_lease.clone(),
            addon_lsp: self.addon_lsp.clone(),
            addon_lsp_claims: self.addon_lsp_claims.clone(),
            addon_hooks: self.addon_hooks.clone(),
        }
    }
    pub(crate) fn addon_base_snapshot(&self) -> Self {
        let snapshot = self.snapshot();
        snapshot
            .functions
            .write()
            .retain(|_, f| f.source != FunctionSource::Workspace);
        snapshot
    }
    pub(crate) fn scoped_functions(
        &self,
        global: Option<&crate::storage::persistence::Db>,
        workspace: Option<(
            &crate::storage::persistence::Db,
            &crate::storage::versioning::VersionManager,
        )>,
    ) -> DaemonResult<Self> {
        let snapshot = self.snapshot();
        snapshot.functions.write().retain(|_, f| {
            f.source == FunctionSource::Builtin
                || (global.is_none() && f.source == FunctionSource::Global)
        });
        if let Some(db) = global {
            snapshot.load_functions(db, FunctionSource::Global)?;
        }
        if let Some((db, versions)) = workspace {
            for mut entry in crate::registry::library::load_all(db)? {
                entry.source = FunctionSource::Workspace;
                if crate::storage::blueprint_files::binding(db, entry.body.id)?.is_some() {
                    entry.body =
                        crate::storage::blueprint_files::load(db, versions, entry.body.id)?;
                    entry.signature =
                        FunctionEntry::derive_signature(&entry.body).map_err(DaemonError::Addon)?;
                }
                snapshot.register_function(entry);
            }
        }
        Ok(snapshot)
    }
    /// Canonical immutable evidence used by checkpoints for non-builtin functions.
    pub fn function_identities(&self) -> std::collections::BTreeMap<String, String> {
        self.functions()
            .into_iter()
            .filter(|f| f.source != FunctionSource::Builtin)
            .map(|f| (f.name.clone(), crate::addon::functions::digest(&f)))
            .collect()
    }
    pub(crate) fn replace_owned_tools(
        &self,
        owner: &str,
        additions: Vec<Arc<dyn Tool>>,
    ) -> DaemonResult<()> {
        let mut names = std::collections::HashSet::new();
        let mut tools = self.tools.write();
        for tool in &additions {
            let name = tool.name();
            if !tool::is_valid_tool_name(name)
                || !names.insert(name.to_string())
                || self.nodes.get(name).is_some()
                || self.functions.read().contains_key(name)
                || (tools.values.contains_key(name)
                    && tools.owners.get(name).map(String::as_str) != Some(owner))
            {
                return Err(DaemonError::Addon(format!(
                    "addon tool registration conflicts: {name}"
                )));
            }
        }
        Self::remove_owned_locked(&mut tools, owner);
        for tool in additions {
            tools.owners.insert(tool.name().into(), owner.into());
            tools.values.insert(tool.name().into(), tool);
        }
        Ok(())
    }
    fn remove_owned_locked(tools: &mut Tools, owner: &str) {
        let names: Vec<_> = tools
            .owners
            .iter()
            .filter(|(_, value)| *value == owner)
            .map(|(name, _)| name.clone())
            .collect();
        for name in names {
            tools.owners.remove(&name);
            tools.values.remove(&name);
        }
    }
    pub(crate) fn remove_owned_tools(&self, owner: &str) {
        Self::remove_owned_locked(&mut self.tools.write(), owner);
    }

    /// Registers a function, replacing any existing one with the same name.
    pub fn register_function(&self, entry: FunctionEntry) {
        self.functions.write().insert(entry.name.clone(), entry);
    }

    pub(crate) fn remove_owned_functions(&mut self,owner:&str) {
        let names:Vec<_>=self.function_owners.iter().filter(|(_,o)|o.as_str()==owner).map(|(n,_)|n.clone()).collect();
        for name in names {self.functions.write().remove(&name);self.function_owners.remove(&name);self.addon_function_bindings.remove(&name);}
    }
    pub(crate) fn register_owned_function(&mut self,owner:&str,entry:FunctionEntry,binding:serde_json::Value)->DaemonResult<()> {
        if self.function(&entry.name).is_some() || self.function_by_id(entry.id).is_some() || self.node_executor(&entry.name).is_some() || self.tool(&entry.name).is_some() {
            return Err(DaemonError::Addon("Addon function name or identity conflicts".into()));
        }
        self.function_owners.insert(entry.name.clone(),owner.into());self.addon_function_bindings.insert(entry.name.clone(),binding);self.register_function(entry);Ok(())
    }

    /// Removes a function by name, returning it when registered.
    pub fn unregister_function(&self, name: &str) -> Option<FunctionEntry> {
        self.functions.write().remove(name)
    }

    /// Returns a function by name.
    pub fn function(&self, name: &str) -> Option<FunctionEntry> {
        self.functions.read().get(name).cloned()
    }

    /// Returns a function by its stable id.
    pub fn function_by_id(&self, id: uuid::Uuid) -> Option<FunctionEntry> {
        self.functions.read().values().find(|f| f.id == id).cloned()
    }

    /// Returns all registered functions, sorted by name.
    pub fn functions(&self) -> Vec<FunctionEntry> {
        let mut out: Vec<FunctionEntry> = self.functions.read().values().cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Registers all functions stored in `db`, tagged with `source`.
    pub fn load_functions(
        &self,
        db: &crate::storage::persistence::Db,
        source: FunctionSource,
    ) -> DaemonResult<Vec<String>> {
        let mut names = Vec::new();
        for mut entry in crate::registry::library::load_all(db)? {
            entry.source = source;
            names.push(entry.name.clone());
            self.register_function(entry);
        }
        Ok(names)
    }

    /// Re-registers the stored function named `name` under `source`.
    ///
    /// Used when a workspace that shadowed a global function closes: the
    /// global definition must come back into the registry. Returns whether a
    /// stored entry was found.
    pub fn restore_function(
        &self,
        db: &crate::storage::persistence::Db,
        name: &str,
        source: FunctionSource,
    ) -> DaemonResult<bool> {
        for mut entry in crate::registry::library::load_all(db)? {
            if entry.name == name {
                entry.source = source;
                self.register_function(entry);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Returns the node executor for the given kind, if registered.
    pub fn node_executor(&self, kind: &str) -> Option<&dyn NodeExecutor> {
        self.nodes.get(kind)
    }

    pub(crate) fn replace_owned_nodes(&mut self, owner:&str, additions:Vec<Arc<dyn NodeExecutor>>, binding:serde_json::Value) -> DaemonResult<()> {
        for node in &additions {
            if self.tools.read().values.contains_key(node.kind()) || self.functions.read().contains_key(node.kind()) {
                return Err(DaemonError::Addon("Addon node conflicts with a tool or function".into()));
            }
        }
        Arc::make_mut(&mut self.nodes).replace_owned(owner,additions,binding)
    }

    /// Returns all registered node kinds.
    pub fn node_kinds(&self) -> Vec<String> {
        self.nodes.kinds()
    }

    pub(crate) fn validate_addon_nodes(&self, blueprint:&metteur_shared::Blueprint) -> DaemonResult<()> {
        let report=metteur_shared::model::validate::validate_with_catalog(blueprint,&self.node_signatures());
        for error in report.errors {
            if matches!(error,metteur_shared::model::validate::BlueprintError::AddonContract{..}) {
                return Err(DaemonError::Addon(error.to_string()));
            }
        }
        Ok(())
    }

    /// Canonical signatures of the executors actually installed in this registry.
    pub fn node_signatures(&self) -> metteur_shared::node_catalog::NodeCatalog {
        let mut catalog = self.nodes.signatures();
        catalog.functions =
            self.functions().into_iter().map(|entry| (entry.name, entry.signature)).collect();
        catalog.addon_function_bindings=self.addon_function_bindings.clone();
        catalog
    }

    /// Authoring snapshot, including registered tools as schema-derived aliases
    /// for Tool nodes. Clone the tool handles under the read lock before calling
    /// their schema methods so concurrent registration cannot deadlock a reader.
    pub fn authoring_catalog(&self) -> metteur_shared::node_catalog::NodeCatalog {
        use metteur_shared::node_catalog::{NodeSignature, PinSignature};
        use metteur_shared::{DataType, NodeType, PinType};
        let mut catalog = self.node_signatures();
        for tool in self.tools() {
            if catalog.contains_key(tool.name()) {
                continue;
            }
            let schema = tool.parameters();
            let required = schema.get("required").and_then(|v| v.as_array());
            let mut pins = catalog
                .get("Tool")
                .map(|s| {
                    s.pins
                        .iter()
                        .filter(|p| p.pin_type != PinType::DataInput)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                for (name, property) in properties {
                    let data_type = match property.get("type").and_then(|v| v.as_str()) {
                        Some("string") => DataType::String,
                        Some("integer") => DataType::Int,
                        Some("number") => DataType::Float,
                        Some("boolean") => DataType::Bool,
                        Some("array") => DataType::List(Box::new(DataType::Any)),
                        Some("object") => DataType::Json,
                        _ => DataType::Any,
                    };
                    pins.push(PinSignature {
                        key: name.clone(),
                        name: name.clone(),
                        pin_type: PinType::DataInput,
                        data_type,
                        default: property.get("default").cloned(),
                        optional: !required
                            .is_some_and(|names| names.iter().any(|v| v.as_str() == Some(name))),
                        choices: property
                            .get("enum")
                            .and_then(|v| v.as_array())
                            .map(|values| {
                                values.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                            })
                            .unwrap_or_default(),
                        description: property
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    });
                }
            }
            catalog.insert(
                tool.name().to_string(),
                NodeSignature {
                    kind: tool.name().to_string(),
                    executor_kind: "Tool".to_string(),
                    node_type: NodeType::Function,
                    pins,
                    dynamic_pins: false,
                    description: tool.description().to_string(),
                },
            );
        }
        catalog
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::DaemonResult;
    use crate::execution::context::ExecutionContext;
    use async_trait::async_trait;
    use metteur_shared::Value;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "Dummy"
        }
        fn description(&self) -> &str {
            "dummy"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn call(&self, _args: &[Value], _ctx: &mut ExecutionContext) -> DaemonResult<Value> {
            Ok(Value::Null)
        }
    }

    #[test]
    fn dynamic_register_and_unregister() {
        let registry = Registry::with_builtins();
        assert!(registry.tool("Dummy").is_none());
        registry.register_tool(Arc::new(DummyTool));
        assert!(registry.tool("Dummy").is_some());
        assert!(registry.unregister_tool("Dummy").is_some());
        assert!(registry.tool("Dummy").is_none());
        assert!(registry.unregister_tool("Dummy").is_none());
    }

    #[test]
    fn builtin_tools_are_registered() {
        let registry = Registry::with_builtins();
        for name in ["ReadFile", "WriteFile", "ExecuteCommand", "SpawnSubAgent"] {
            assert!(registry.tool(name).is_some(), "{name} should be registered");
        }
    }

    #[test]
    fn non_pascal_tool_names_are_rejected() {
        struct BadTool(&'static str);

        #[async_trait]
        impl Tool for BadTool {
            fn name(&self) -> &str {
                self.0
            }
            fn description(&self) -> &str {
                "bad"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            async fn call(
                &self,
                _args: &[Value],
                _ctx: &mut ExecutionContext,
            ) -> DaemonResult<Value> {
                Ok(Value::Null)
            }
        }

        let registry = Registry::with_builtins();
        for bad in ["snake_case", "kebab-case", "lower", "", "Has Space", "GitStatus_2"] {
            assert!(
                registry.try_register_tool(Arc::new(BadTool(bad))).is_err(),
                "{bad} should be rejected"
            );
        }
        assert!(registry.tool("snake_case").is_none());
    }
}
