//! Node executor trait and registry.

use std::collections::HashMap;

use async_trait::async_trait;
use metteur_shared::{Node, PinId, Value};

use crate::error::DaemonResult;
use crate::execution::context::ExecutionContext;

/// Executes a single node given its data input values.
///
/// Implementations return the values produced on the node's data output pins,
/// keyed by pin id. The shared [`ExecutionContext`] provides access to the
/// registry, LLM clients, audit logging and the transaction log.
#[async_trait]
pub trait NodeExecutor: Send + Sync {
    /// The node kind this executor handles (e.g. `Start`, `Add`, `Branch`).
    fn kind(&self) -> &str;

    /// Canonical contract adopted by this executor. Custom executors override
    /// this method when they expose fixed pins; the default explicitly marks
    /// an instance-defined contract rather than inventing a pin layout.
    fn signature(&self) -> metteur_shared::node_catalog::NodeSignature {
        use metteur_shared::node_catalog::{NodeSignature, builtin_signature};
        builtin_signature(self.kind()).unwrap_or_else(|| NodeSignature {
            kind: self.kind().to_string(),
            executor_kind: self.kind().to_string(),
            node_type: metteur_shared::NodeType::Function,
            pins: Vec::new(),
            dynamic_pins: true,
            description: "Custom executor: pins are defined by the node instance.".to_string(),
        })
    }

    /// Executes the node and returns its data output values.
    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>>;
}

/// A registry of node executors keyed by node kind.
#[derive(Default, Clone)]
pub struct NodeRegistry {
    executors: HashMap<String, std::sync::Arc<dyn NodeExecutor>>,
    owners: HashMap<String, String>,
    bindings: std::collections::BTreeMap<String, serde_json::Value>,
}

impl NodeRegistry {
    /// Creates a registry pre-populated with the built-in executors.
    pub fn with_builtins() -> Self {
        use crate::execution::nodes as n;
        let mut registry = Self::default();
        registry.register(Box::new(n::StartExecutor));
        registry.register(Box::new(n::EndExecutor));
        registry.register(Box::new(n::AddExecutor));
        registry.register(Box::new(n::SubtractExecutor));
        registry.register(Box::new(n::MultiplyExecutor));
        registry.register(Box::new(n::DivideExecutor));
        registry.register(Box::new(n::BranchExecutor));
        registry.register(Box::new(n::SwitchExecutor));
        registry.register(Box::new(n::ForEachExecutor));
        registry.register(Box::new(n::CallLlmExecutor));
        registry.register(Box::new(n::ToolExecutor));
        registry.register(Box::new(n::ValidatorExecutor));
        registry.register(Box::new(n::JudgeExecutor));
        registry.register(Box::new(n::LspCheckExecutor));
        registry.register(Box::new(n::AbstractExecutor));
        registry.register(Box::new(n::CallFunctionExecutor));
        registry.register(Box::new(n::FunctionEntryExecutor));
        registry.register(Box::new(n::FunctionExitExecutor));
        // Extended math.
        registry.register(Box::new(n::ModuloExecutor));
        registry.register(Box::new(n::PowerExecutor));
        registry.register(Box::new(n::MinExecutor));
        registry.register(Box::new(n::MaxExecutor));
        registry.register(Box::new(n::AbsExecutor));
        registry.register(Box::new(n::RoundExecutor));
        // Comparison and logic.
        registry.register(Box::new(n::EqualExecutor));
        registry.register(Box::new(n::NotEqualExecutor));
        registry.register(Box::new(n::GreaterExecutor));
        registry.register(Box::new(n::LessExecutor));
        registry.register(Box::new(n::GreaterEqualExecutor));
        registry.register(Box::new(n::LessEqualExecutor));
        registry.register(Box::new(n::AndExecutor));
        registry.register(Box::new(n::OrExecutor));
        registry.register(Box::new(n::XorExecutor));
        registry.register(Box::new(n::NotExecutor));
        // Strings and conversion.
        registry.register(Box::new(n::ConcatExecutor));
        registry.register(Box::new(n::LengthExecutor));
        registry.register(Box::new(n::UpperExecutor));
        registry.register(Box::new(n::LowerExecutor));
        registry.register(Box::new(n::TrimExecutor));
        registry.register(Box::new(n::ContainsExecutor));
        registry.register(Box::new(n::ReplaceExecutor));
        registry.register(Box::new(n::SubstringExecutor));
        registry.register(Box::new(n::ToStringExecutor));
        registry.register(Box::new(n::ToIntExecutor));
        registry.register(Box::new(n::ToFloatExecutor));
        registry.register(Box::new(n::ToBoolExecutor));
        registry.register(Box::new(n::ToJsonExecutor));
        registry.register(Box::new(n::ParseJsonExecutor));
        // Collections.
        registry.register(Box::new(n::ListCreateExecutor));
        registry.register(Box::new(n::ListAppendExecutor));
        registry.register(Box::new(n::ListGetExecutor));
        registry.register(Box::new(n::ListLengthExecutor));
        registry.register(Box::new(n::ListContainsExecutor));
        registry.register(Box::new(n::JsonGetExecutor));
        registry.register(Box::new(n::JsonSetExecutor));
        // Context manager nodes.
        registry.register(Box::new(n::ContextCreateExecutor));
        registry.register(Box::new(n::ContextCloneExecutor));
        registry.register(Box::new(n::ContextMergeExecutor));
        registry.register(Box::new(n::ContextFilterExecutor));
        registry.register(Box::new(n::ContextTrimExecutor));
        registry.register(Box::new(n::ContextReleaseExecutor));
        registry.register(Box::new(n::ContextToTextExecutor));
        // Flow support.
        registry.register(Box::new(n::DelayExecutor));
        registry.register(Box::new(n::RequestApprovalExecutor));
        registry.register(Box::new(n::OversightCheckpointExecutor));
        // Frame-scoped variables.
        registry.register(Box::new(n::VariableSetExecutor));
        registry.register(Box::new(n::VariableGetExecutor));
        registry
    }

    /// Registers an executor, replacing any existing one with the same kind.
    pub fn register(&mut self, executor: Box<dyn NodeExecutor>) {
        self.executors.insert(executor.kind().to_string(), executor.into());
    }

    pub(crate) fn replace_owned(
        &mut self,
        owner: &str,
        additions: Vec<std::sync::Arc<dyn NodeExecutor>>,
        binding: serde_json::Value,
    ) -> DaemonResult<()> {
        let mut names = std::collections::HashSet::new();
        for node in &additions {
            if !names.insert(node.kind().to_string())
                || (self.executors.contains_key(node.kind())
                    && self.owners.get(node.kind()).map(String::as_str) != Some(owner))
            {
                return Err(crate::DaemonError::Addon("Addon node registration conflicts".into()));
            }
        }
        for name in self
            .owners
            .iter()
            .filter(|(_, o)| o.as_str() == owner)
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>()
        {
            self.executors.remove(&name);
            self.owners.remove(&name);
            self.bindings.remove(&name);
        }
        for node in additions {
            let name = node.kind().to_string();
            self.owners.insert(name.clone(), owner.into());
            self.bindings.insert(name.clone(), binding.clone());
            self.executors.insert(name, node);
        }
        Ok(())
    }

    /// Returns the executor for the given node kind, if registered.
    pub fn get(&self, kind: &str) -> Option<&dyn NodeExecutor> {
        self.executors.get(kind).map(|e| e.as_ref())
    }

    /// Returns all registered node kinds.
    pub fn kinds(&self) -> Vec<String> {
        let mut kinds: Vec<String> = self.executors.keys().cloned().collect();
        kinds.sort();
        kinds
    }

    /// Owned, sorted snapshot; readers never retain a mutable registry borrow.
    pub fn signatures(&self) -> metteur_shared::node_catalog::NodeCatalog {
        let mut catalog: metteur_shared::node_catalog::NodeCatalog = self
            .executors
            .iter()
            .map(|(kind, executor)| (kind.clone(), executor.signature()))
            .collect();
        catalog.addon_bindings = self.bindings.clone();
        catalog
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_include_abstract_kind() {
        let registry = NodeRegistry::with_builtins();
        assert!(registry.get("Abstract").is_some());
        assert!(registry.kinds().contains(&"Abstract".to_string()));
    }
}
