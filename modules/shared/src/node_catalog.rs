//! Canonical built-in node contracts, shared by executors and offline DSL tools.
//!
//! A running daemon publishes only its registered executors' contracts. Clients
//! must use that snapshot; this module is not a list of installed capabilities.
use crate::{DataType, NodeType, Pin, PinType};
use std::collections::BTreeMap;
pub mod addon;

/// An immutable, sorted snapshot of the registered node contracts.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NodeCatalog {
    /// Actual registered kinds, sorted by name.
    pub nodes: BTreeMap<String, NodeSignature>,
    /// Dynamic CallFunction contracts from the function library.
    pub functions: BTreeMap<String, crate::FunctionSignature>,
    /// Verified immutable package identity, persisted on newly authored nodes.
    #[serde(default)]
    pub addon_bindings: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub addon_function_bindings: BTreeMap<String, serde_json::Value>,
}

impl std::ops::Deref for NodeCatalog {
    type Target = BTreeMap<String, NodeSignature>;
    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}

impl std::ops::DerefMut for NodeCatalog {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.nodes
    }
}

impl FromIterator<(String, NodeSignature)> for NodeCatalog {
    fn from_iter<T: IntoIterator<Item = (String, NodeSignature)>>(iter: T) -> Self {
        Self {
            nodes: iter.into_iter().collect(),
            functions: BTreeMap::new(),
            addon_bindings: BTreeMap::new(),
            addon_function_bindings: BTreeMap::new(),
        }
    }
}

impl<'a> IntoIterator for &'a NodeCatalog {
    type Item = (&'a String, &'a NodeSignature);
    type IntoIter = std::collections::btree_map::Iter<'a, String, NodeSignature>;
    fn into_iter(self) -> Self::IntoIter {
        self.nodes.iter()
    }
}

impl NodeCatalog {
    /// Resolves fixed executor pins plus the current function library signature.
    pub fn resolve(&self, kind: &str, data: &serde_json::Value) -> Option<NodeSignature> {
        let mut signature = self.get(kind)?.clone();
        if kind == "CallFunction"
            && let Some(function) = data
                .get("function")
                .and_then(|v| v.as_str())
                .and_then(|name| self.functions.get(name))
        {
            for (pins, pin_type) in
                [(&function.inputs, PinType::DataInput), (&function.outputs, PinType::DataOutput)]
            {
                signature.pins.extend(pins.iter().map(|pin| PinSignature {
                    key: pin.name.clone(),
                    name: pin.name.clone(),
                    pin_type,
                    data_type: pin.data_type.clone(),
                    default: pin.default.clone(),
                    optional: pin.optional,
                    description: pin.description.clone(),
                    choices: Vec::new(),
                }));
            }
        }
        Some(signature)
    }
}

/// A node contract. Function and tool instances may add dynamically derived pins.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NodeSignature {
    /// Kind advertised to an author.
    pub kind: String,
    /// Executor kind (tool aliases resolve to Tool).
    pub executor_kind: String,
    /// Execution category.
    pub node_type: NodeType,
    /// Ordered canonical pins, without instance ids.
    pub pins: Vec<PinSignature>,
    /// Whether instance/library/schema data defines additional pins.
    pub dynamic_pins: bool,
    /// Description of instance-specific pin and configuration rules.
    pub description: String,
}

impl NodeSignature {
    /// Matches an existing pin without relying on its instance id or type.
    pub fn pin(&self, pin: &Pin) -> Option<&PinSignature> {
        self.pins.iter().find(|p| {
            p.pin_type == pin.pin_type
                && (p.name == pin.name
                    || (!p.key.is_empty() && pin.key.as_deref() == Some(p.key.as_str())))
        })
    }
}

impl Default for PinSignature {
    fn default() -> Self {
        Self {
            key: String::new(),
            name: String::new(),
            pin_type: PinType::DataInput,
            data_type: DataType::Any,
            default: None,
            optional: false,
            choices: Vec::new(),
            description: None,
        }
    }
}

impl PinSignature {
    /// Instantiates metadata with a caller-selected pin id.
    pub fn instantiate(&self, id: uuid::Uuid) -> Pin {
        Pin {
            id,
            key: (!self.key.is_empty()).then(|| self.key.clone()),
            name: self.name.clone(),
            pin_type: self.pin_type,
            data_type: self.data_type.clone(),
            default: self.default.clone(),
            optional: self.optional,
            choices: self.choices.clone(),
            description: self.description.clone(),
        }
    }
}

/// Inline input lookup used by validation and execution. Edges take precedence;
/// then name, semantic key and instance id; finally the pin's default.
pub fn inline_value<'a>(node: &'a crate::Node, pin: &'a Pin) -> Option<&'a serde_json::Value> {
    node.data
        .get(&pin.name)
        .or_else(|| pin.key.as_deref().and_then(|key| node.data.get(key)))
        .or_else(|| node.data.get(pin.id.to_string()))
}

/// Offline built-in catalogue. Live services pass their actual registry snapshot.
pub fn builtin_catalog() -> NodeCatalog {
    known_kinds()
        .into_iter()
        .filter_map(|kind| builtin_signature(kind).map(|s| (kind.to_string(), s)))
        .collect()
}

/// The contract adopted by a built-in executor (and reused by offline compilers).
pub fn builtin_signature(kind: &str) -> Option<NodeSignature> {
    let mut pins = template(kind)?;
    for pin in &mut pins {
        if pin.pin_type == PinType::DataInput {
            pin.description = Some(format!(
                "{} input; inline values use node.data[{}] (name and pin id are also accepted).",
                pin.name, pin.key
            ));
            if kind == "CallLLM"
                || kind == "ListCreate"
                || (kind == "ContextRelease" && pin.name != "Context")
            {
                pin.optional = true;
            }
            if kind == "LspCheck" && pin.name == "TimeoutMs" {
                pin.default = Some(serde_json::json!(5000));
                pin.optional = true;
            }
            if pin.name == "ReasoningEffort" {
                pin.choices = ["none", "low", "medium", "high"].map(String::from).to_vec();
            }
            if pin.name == "Role" {
                pin.choices = ["user", "assistant", "tool", "system"].map(String::from).to_vec();
            }
        }
    }
    let tool = REGISTRY_TOOL_KINDS.contains(&kind);
    // ToolExecutor serializes every tool result to text, regardless of tool value type.
    if tool {
        for pin in &mut pins {
            if pin.pin_type == PinType::DataOutput {
                pin.data_type = DataType::String;
            }
        }
    }
    let description = match kind {
        "Start" => "Additional data outputs come from node.data constants.",
        "Switch" => "Additional Case_<value> execution outputs come from node.data.cases.",
        "FunctionEntry" | "FunctionExit" | "CallFunction" => {
            "Data pins come from the function library signature; instance pins are preserved."
        }
        "Tool" => "tool_name selects a registered tool; argument pins come from its JSON schema.",
        "Abstract" => {
            "No fixed data pins; expansion is configured in node.data (prompt, model, max_expand_depth)."
        }
        "Validator" => {
            "node.data.mode selects validation; expected, regex and retry retain their existing configuration meanings."
        }
        "Judge" => {
            "Score with node.data.threshold (default 1), or Result with node.data.success_criteria."
        }
        "LspCheck" => "Path or node.data.paths; node.data.skip_if_unavailable defaults to true.",
        "OversightCheckpoint" => {
            "node.data.async defaults to false. Async returns pending; synchronous gates preserve the actual review verdict and require disposition on failure or unhandled concern."
        }
        "ContextRelease" => {
            "Select Paths, Patterns, Tools, or node.data.all; node.data.keep_recent is retained."
        }
        "CallLLM" => {
            "Connected inputs override node.data; absent generation options inherit workspace defaults."
        }
        _ => "Connected data inputs override inline node.data values, then pin defaults.",
    };
    Some(NodeSignature {
        kind: kind.to_string(),
        executor_kind: if tool {
            "Tool"
        } else {
            kind
        }
        .to_string(),
        node_type: node_type_of(kind),
        pins,
        dynamic_pins: matches!(
            kind,
            "Start"
                | "Switch"
                | "Tool"
                | "Abstract"
                | "FunctionEntry"
                | "FunctionExit"
                | "CallFunction"
        ),
        description: description.to_string(),
    })
}

/// A pin template entry.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PinSignature {
    /// Semantic key (matches canvas/node-data keys); empty for exec pins.
    pub key: String,
    /// Display/wire name read by the executor.
    pub name: String,
    /// Execution/data direction.
    pub pin_type: PinType,
    /// Type accepted or produced by the executor.
    pub data_type: DataType,
    /// Fallback value for an unconnected input.
    pub default: Option<serde_json::Value>,
    /// Whether the executor accepts an absent input.
    pub optional: bool,
    /// Allowed string choices.
    pub choices: Vec<String>,
    /// Human-readable pin documentation.
    pub description: Option<String>,
}

/// Maps the template's canonical wire name to the pin's semantic key (the key
/// canvas node data and the DSL init block use).
pub fn key_of(name: &str) -> String {
    let key = match name {
        "A" => "a",
        "B" => "b",
        "In" => "in",
        "Result" => "result",
        "Context" => "context",
        "Text" => "text",
        "Role" => "role",
        "System" => "system",
        "Prompt" => "prompt",
        "ReasoningEffort" => "reasoning_effort",
        "Model" => "model",
        "Temperature" => "temperature",
        "TopP" => "top_p",
        "MaxTokens" => "max_tokens",
        "MaxIterations" => "max_iterations",
        "Seed" => "seed",
        "Input" => "input",
        "Find" => "find",
        "ReplaceWith" => "replaceWith",
        "Start" => "start",
        "Length" => "length",
        "Keep" => "keep",
        "List" => "list",
        "Item" => "item",
        "Index" => "index",
        "Object" => "object",
        "Path" => "path",
        "Value" => "value",
        "ItemA" => "itemA",
        "ItemB" => "itemB",
        "ItemC" => "itemC",
        "Ms" => "ms",
        "Message" => "message",
        "Allowed" => "allowed",
        "ToolName" => "tool_name",
        "Command" => "command",
        "Condition" => "cond",
        "Actual" => "actual",
        "Expected" => "expected",
        "Passed" => "passed",
        "Score" => "score",
        "Success" => "success",
        "Errors" => "errors",
        "Warnings" => "warnings",
        "Diagnostics" => "diagnostics",
        "TimeoutMs" => "timeout_ms",
        other => return other.to_ascii_lowercase(),
    };
    key.to_string()
}

/// Returns the pin layout for a known node kind, if DataType::Any.
/// Registry tool names the DSL accepts as node kinds.
///
/// Each one compiles to a `Tool` node whose `data.tool_name` selects the
/// registered tool; the shared crate cannot query the daemon registry, so the
/// list is explicit and grows with the tool set.
pub const REGISTRY_TOOL_KINDS: &[&str] = &[
    "ReadFile",
    "WriteFile",
    "EditFile",
    "ListDirectory",
    "SearchFile",
    "Grep",
    "Glob",
    "ExecuteCommand",
    "StartCommand",
    "JobStatus",
    "WaitJob",
    "KillJob",
    "GetDependencies",
    "CheckDiagnostics",
    "GetHover",
    "FindDefinition",
    "ReleaseContext",
];

/// Returns every node kind the DSL and draft formats accept.
///
/// Used for "did you mean" hints; the list is the union of the template table
/// and the registry tool kinds. A template that answers for a probe kind is
/// what defines membership, so this list cannot drift from [`template`].
pub fn known_kinds() -> Vec<&'static str> {
    const CANDS: &[&str] = &[
        "Start",
        "End",
        "Add",
        "Subtract",
        "Multiply",
        "Divide",
        "Modulo",
        "Power",
        "Min",
        "Max",
        "Abs",
        "Round",
        "Equal",
        "NotEqual",
        "Greater",
        "Less",
        "GreaterEqual",
        "LessEqual",
        "And",
        "Or",
        "Xor",
        "Not",
        "Concat",
        "Length",
        "Upper",
        "Lower",
        "Trim",
        "Contains",
        "Replace",
        "Substring",
        "ToString",
        "ToInt",
        "ToFloat",
        "ToBool",
        "ToJson",
        "ParseJson",
        "ListCreate",
        "ListAppend",
        "ListGet",
        "ListLength",
        "ListContains",
        "JsonGet",
        "JsonSet",
        "ContextCreate",
        "ContextClone",
        "ContextMerge",
        "ContextFilter",
        "ContextTrim",
        "ContextRelease",
        "ContextToText",
        "Delay",
        "VariableGet",
        "VariableSet",
        "Branch",
        "Switch",
        "ForEach",
        "RequestApproval",
        "OversightCheckpoint",
        "CallLLM",
        "Tool",
        "Validator",
        "Judge",
        "LspCheck",
        "Abstract",
        "CallFunction",
        "FunctionEntry",
        "FunctionExit",
    ];
    let mut kinds: Vec<&str> = CANDS
        .iter()
        .copied()
        .filter(|kind| template(kind).is_some())
        .chain(REGISTRY_TOOL_KINDS.iter().copied())
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

fn template(kind: &str) -> Option<Vec<PinSignature>> {
    let i = |n: &'static str| PinSignature {
        key: String::new(),
        name: n.to_string(),
        pin_type: PinType::ExecInput,
        data_type: DataType::Void,
        ..Default::default()
    };
    let o = |n: &'static str| PinSignature {
        key: String::new(),
        name: n.to_string(),
        pin_type: PinType::ExecOutput,
        data_type: DataType::Void,
        ..Default::default()
    };
    let di = |n: &'static str, dt: DataType| PinSignature {
        key: key_of(n),
        name: n.to_string(),
        pin_type: PinType::DataInput,
        data_type: dt,
        ..Default::default()
    };
    let do_ = |n: &'static str, dt: DataType| PinSignature {
        key: key_of(n),
        name: n.to_string(),
        pin_type: PinType::DataOutput,
        data_type: dt,
        ..Default::default()
    };
    // Exec pins are named `x-in`/`x-out` to match the canvas convention.
    let exec = |exec_in: bool| {
        let mut pins = Vec::new();
        if exec_in {
            pins.push(i("x-in"));
            pins.push(o("x-out"));
        }
        pins
    };
    // Two numeric/typed inputs + one `Result` output.
    let bin = |a: DataType, b: DataType, r: DataType| {
        let mut pins = exec(true);
        pins.push(di("A", a));
        pins.push(di("B", b));
        pins.push(do_("Result", r));
        pins
    };
    // One `In` input + one `Result` output.
    let un = |i: DataType, r: DataType| {
        let mut pins = exec(true);
        pins.push(di("In", i));
        pins.push(do_("Result", r));
        pins
    };
    Some(match kind {
        "Start" => vec![o("x-out"), do_("Context", DataType::Context)],
        "End" => vec![i("x-in")],
        "Add" | "Subtract" | "Multiply" | "Divide" | "Modulo" | "Power" | "Min" | "Max" => {
            bin(DataType::Float, DataType::Float, DataType::Float)
        }
        "Abs" | "Round" => {
            vec![i("x-in"), o("x-out"), di("A", DataType::Float), do_("Result", DataType::Float)]
        }
        "Equal" | "NotEqual" => bin(DataType::Any, DataType::Any, DataType::Bool),
        "Greater" | "Less" | "GreaterEqual" | "LessEqual" => {
            bin(DataType::Any, DataType::Any, DataType::Bool)
        }
        "And" | "Or" | "Xor" => bin(DataType::Bool, DataType::Bool, DataType::Bool),
        "Not" => {
            vec![i("x-in"), o("x-out"), di("A", DataType::Bool), do_("Result", DataType::Bool)]
        }
        "Concat" => bin(DataType::String, DataType::String, DataType::String),
        "Length" => un(DataType::String, DataType::Float),
        "Upper" | "Lower" | "Trim" => un(DataType::String, DataType::String),
        "Contains" => bin(DataType::String, DataType::String, DataType::Bool),
        "Replace" => {
            let mut pins = exec(true);
            pins.push(di("Input", DataType::String));
            pins.push(di("Find", DataType::String));
            pins.push(di("ReplaceWith", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "Substring" => {
            let mut pins = exec(true);
            pins.push(di("In", DataType::String));
            pins.push(di("Start", DataType::Int));
            pins.push(di("Length", DataType::Int));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "ToString" => un(DataType::Any, DataType::String),
        "ToInt" => un(DataType::Any, DataType::Int),
        "ToFloat" => un(DataType::Any, DataType::Float),
        "ToBool" => un(DataType::Any, DataType::Bool),
        "ToJson" => un(DataType::Any, DataType::Json),
        "ParseJson" => un(DataType::String, DataType::Json),
        "ListCreate" => {
            let mut pins = exec(true);
            for item in ["ItemA", "ItemB", "ItemC"] {
                pins.push(di(item, DataType::Any));
            }
            pins.push(do_("Result", DataType::List(Box::new(DataType::Any))));
            pins
        }
        "ListAppend" => {
            let mut pins = exec(true);
            pins.push(di("List", DataType::List(Box::new(DataType::Any))));
            pins.push(di("Item", DataType::Any));
            pins.push(do_("Result", DataType::List(Box::new(DataType::Any))));
            pins
        }
        "ListGet" => {
            let mut pins = exec(true);
            pins.push(di("List", DataType::List(Box::new(DataType::Any))));
            pins.push(di("Index", DataType::Int));
            pins.push(do_("Result", DataType::Any));
            pins
        }
        "ListLength" => {
            let mut pins = exec(true);
            pins.push(di("List", DataType::List(Box::new(DataType::Any))));
            pins.push(do_("Result", DataType::Float));
            pins
        }
        "ListContains" => {
            let mut pins = exec(true);
            pins.push(di("List", DataType::List(Box::new(DataType::Any))));
            pins.push(di("Item", DataType::Any));
            pins.push(do_("Result", DataType::Bool));
            pins
        }
        "JsonGet" => {
            let mut pins = exec(true);
            pins.push(di("Object", DataType::Json));
            pins.push(di("Path", DataType::String));
            pins.push(do_("Result", DataType::Any));
            pins
        }
        "JsonSet" => {
            let mut pins = exec(true);
            pins.push(di("Object", DataType::Json));
            pins.push(di("Path", DataType::String));
            pins.push(di("Value", DataType::Any));
            pins.push(do_("Result", DataType::Json));
            pins
        }
        "ContextCreate" => {
            let mut pins = exec(true);
            pins.push(di("System", DataType::String));
            pins.push(di("Prompt", DataType::String));
            pins.push(do_("Result", DataType::Context));
            pins
        }
        "ContextClone" => un(DataType::Context, DataType::Context),
        "ContextMerge" => {
            let mut pins = exec(true);
            pins.push(di("Context", DataType::Context));
            pins.push(di("Text", DataType::String));
            pins.push(di("Role", DataType::Choice));
            pins.push(do_("Result", DataType::Context));
            pins
        }
        "ContextFilter" => {
            let mut pins = exec(true);
            pins.push(di("Context", DataType::Context));
            pins.push(di("Role", DataType::Choice));
            pins.push(do_("Result", DataType::Context));
            pins
        }
        "ContextTrim" => {
            let mut pins = exec(true);
            pins.push(di("Context", DataType::Context));
            pins.push(di("Keep", DataType::Int));
            pins.push(do_("Result", DataType::Context));
            pins
        }
        "ContextRelease" => {
            let mut pins = exec(true);
            pins.push(di("Context", DataType::Context));
            pins.push(di("Paths", DataType::List(Box::new(DataType::String))));
            pins.push(di("Patterns", DataType::List(Box::new(DataType::String))));
            pins.push(di("Tools", DataType::List(Box::new(DataType::String))));
            pins.push(do_("Result", DataType::Context));
            pins.push(do_("Released", DataType::Int));
            pins.push(do_("Freed", DataType::Int));
            pins
        }
        "ContextToText" => un(DataType::Context, DataType::String),
        "Delay" => {
            let mut pins = exec(true);
            pins.push(di("Ms", DataType::Int));
            pins
        }
        "Branch" => {
            let mut pins = vec![i("x-in")];
            pins.push(PinSignature {
                key: String::new(),
                name: "True".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(PinSignature {
                key: String::new(),
                name: "False".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(di("Condition", DataType::Any));
            pins.push(do_("Result", DataType::Bool));
            pins
        }
        "Switch" => {
            let mut pins = vec![i("x-in")];
            pins.push(PinSignature {
                key: String::new(),
                name: "Default".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(di("Case", DataType::Any));
            pins.push(do_("Result", DataType::Any));
            pins
        }
        "ForEach" => {
            let mut pins = vec![i("x-in")];
            pins.push(PinSignature {
                key: String::new(),
                name: "Body".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(PinSignature {
                key: String::new(),
                name: "Completed".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(di("List", DataType::List(Box::new(DataType::Any))));
            pins.push(do_("Iteration", DataType::Any));
            pins.push(do_("Index", DataType::Int));
            pins
        }
        "VariableSet" => {
            let mut pins = exec(true);
            pins.push(di("Name", DataType::String));
            pins.push(di("Value", DataType::Any));
            pins.push(do_("Value", DataType::Any));
            pins
        }
        "VariableGet" => {
            let mut pins = exec(true);
            pins.push(di("Name", DataType::String));
            pins.push(do_("Value", DataType::Any));
            pins
        }
        "CallLLM" => {
            let mut pins = exec(true);
            pins.push(do_("Result", DataType::String));
            pins.push(do_("Context", DataType::Context));
            pins.push(di("Context", DataType::Context));
            pins.push(di("ReasoningEffort", DataType::Choice));
            pins.push(di("Model", DataType::String));
            pins.push(di("Prompt", DataType::String));
            pins.push(di("System", DataType::String));
            pins.push(di("Temperature", DataType::Float));
            pins.push(di("TopP", DataType::Float));
            pins.push(di("MaxTokens", DataType::Int));
            pins.push(di("MaxIterations", DataType::Int));
            pins.push(di("Seed", DataType::Int));
            pins
        }
        "Validator" => {
            let mut pins = exec(true);
            pins.push(di("Actual", DataType::Any));
            pins.push(di("Expected", DataType::Any));
            pins.push(do_("Passed", DataType::Bool));
            pins
        }
        "Judge" => {
            let mut pins = exec(true);
            pins.push(di("Score", DataType::Float));
            pins.push(di("Result", DataType::Json));
            pins.push(do_("Success", DataType::Bool));
            pins
        }
        "LspCheck" => {
            let mut pins = exec(true);
            pins.push(di("Path", DataType::String));
            pins.push(di("TimeoutMs", DataType::Int));
            pins.push(do_("Passed", DataType::Bool));
            pins.push(do_("Errors", DataType::Int));
            pins.push(do_("Warnings", DataType::Int));
            pins.push(do_("Diagnostics", DataType::String));
            pins.push(do_("Problems", DataType::List(Box::new(DataType::Json))));
            pins
        }
        "OversightCheckpoint" => {
            let mut pins = exec(true);
            for name in ["ReviewId", "Status", "Verdict", "Notes"] {
                pins.push(do_(name, DataType::String));
            }
            pins
        }
        "RequestApproval" => {
            let mut pins = vec![i("x-in")];
            pins.push(PinSignature {
                key: String::new(),
                name: "x-out".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(PinSignature {
                key: String::new(),
                name: "Approved".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(PinSignature {
                key: String::new(),
                name: "Denied".to_string(),
                pin_type: PinType::ExecOutput,
                data_type: DataType::Void,
                ..Default::default()
            });
            pins.push(di("Message", DataType::String));
            pins.push(do_("Allowed", DataType::Bool));
            pins
        }
        "Tool" => {
            let mut pins = exec(true);
            pins.push(di("ToolName", DataType::String));
            pins.push(di("Command", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        // Round 13 search/edit tools: the DSL names them directly and the
        // compiler rewrites the kind to `Tool` with `tool_name` set, so the
        // pins mirror each tool's JSON schema argument names.
        "Grep" => {
            let mut pins = exec(true);
            pins.push(di("pattern", DataType::String));
            pins.push(di("path", DataType::String));
            pins.push(di("glob", DataType::String));
            pins.push(di("case_insensitive", DataType::Bool));
            pins.push(di("context_lines", DataType::Int));
            pins.push(di("max_matches", DataType::Int));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "Glob" => {
            let mut pins = exec(true);
            pins.push(di("pattern", DataType::String));
            pins.push(di("path", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "EditFile" => {
            let mut pins = exec(true);
            pins.push(di("path", DataType::String));
            pins.push(di("edits", DataType::Json));
            pins.push(di("expected_sha256", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "ReadFile" => {
            let mut pins = exec(true);
            pins.push(di("path", DataType::String));
            pins.push(di("offset", DataType::Int));
            pins.push(di("limit", DataType::Int));
            pins.push(di("line_numbers", DataType::Bool));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "WriteFile" => {
            let mut pins = exec(true);
            pins.push(di("path", DataType::String));
            pins.push(di("content", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "ListDirectory" => {
            let mut pins = exec(true);
            pins.push(di("path", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "SearchFile" => {
            let mut pins = exec(true);
            pins.push(di("root", DataType::String));
            pins.push(di("query", DataType::String));
            pins.push(do_("Result", DataType::List(Box::new(DataType::String))));
            pins
        }
        "ExecuteCommand" => {
            let mut pins = exec(true);
            pins.push(di("command", DataType::String));
            pins.push(di("cwd", DataType::String));
            pins.push(di("timeout_secs", DataType::Int));
            pins.push(do_("Result", DataType::Json));
            pins
        }
        "StartCommand" => {
            let mut pins = exec(true);
            pins.push(di("command", DataType::String));
            pins.push(di("cwd", DataType::String));
            pins.push(do_("Result", DataType::Json));
            pins
        }
        "JobStatus" => {
            let mut pins = exec(true);
            pins.push(di("job_id", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "WaitJob" => {
            let mut pins = exec(true);
            pins.push(di("job_id", DataType::String));
            pins.push(di("timeout_secs", DataType::Int));
            pins.push(do_("Result", DataType::Json));
            pins
        }
        "KillJob" => {
            let mut pins = exec(true);
            pins.push(di("job_id", DataType::String));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "GetDependencies" => {
            let mut pins = exec(true);
            pins.push(di("path", DataType::String));
            pins.push(do_("Result", DataType::Json));
            pins
        }
        "CheckDiagnostics" => {
            let mut pins = exec(true);
            pins.push(di("path", DataType::String));
            pins.push(di("timeout_ms", DataType::Int));
            pins.push(do_("Result", DataType::Json));
            pins
        }
        "GetHover" | "FindDefinition" => {
            let mut pins = exec(true);
            pins.push(di("path", DataType::String));
            pins.push(di("line", DataType::Int));
            pins.push(di("character", DataType::Int));
            pins.push(do_("Result", DataType::Json));
            pins
        }
        "ReleaseContext" => {
            let mut pins = exec(true);
            pins.push(di("paths", DataType::List(Box::new(DataType::String))));
            pins.push(di("patterns", DataType::List(Box::new(DataType::String))));
            pins.push(di("tools", DataType::List(Box::new(DataType::String))));
            pins.push(di("all", DataType::Bool));
            pins.push(di("keep_recent", DataType::Int));
            pins.push(do_("Result", DataType::String));
            pins
        }
        "Abstract" => exec(true),
        "FunctionEntry" => vec![o("x-out")],
        "FunctionExit" => vec![i("x-in")],
        "CallFunction" => exec(true),
        _ => return None,
    })
}

/// Returns the node category a kind belongs to for compiled blueprints.
pub fn node_type_of(kind: &str) -> NodeType {
    match kind {
        "Start" | "End" | "FunctionEntry" | "FunctionExit" => NodeType::Event,
        "Branch"
        | "Switch"
        | "ForEach"
        | "RequestApproval"
        | "LspCheck"
        | "OversightCheckpoint" => NodeType::Control,
        "Add" | "Subtract" | "Multiply" | "Divide" | "Modulo" | "Power" | "Min" | "Max" | "Abs"
        | "Round" | "Equal" | "NotEqual" | "Greater" | "Less" | "GreaterEqual" | "LessEqual"
        | "And" | "Or" | "Xor" | "Not" | "Concat" | "Length" | "Upper" | "Lower" | "Trim"
        | "Contains" | "Replace" | "Substring" | "ToString" | "ToInt" | "ToFloat" | "ToBool"
        | "ToJson" | "ParseJson" | "ListCreate" | "ListAppend" | "ListGet" | "ListLength"
        | "ListContains" | "JsonGet" | "JsonSet" | "ContextCreate" | "ContextClone"
        | "ContextMerge" | "ContextFilter" | "ContextTrim" | "ContextRelease" | "ContextToText"
        | "Delay" | "VariableGet" => NodeType::Pure,
        _ => NodeType::Function,
    }
}
