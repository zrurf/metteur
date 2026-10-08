//! Blueprint function library model.
//!
//! A function is a callable [`Blueprint`] body wrapped by a fixed entry/exit
//! node pair from which its [`FunctionSignature`] is derived.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::blueprint::{Blueprint, DataType, Node, PinType};

/// The node kind of a function body's entry node.
pub const FUNCTION_ENTRY_KIND: &str = "FunctionEntry";
/// The node kind of a function body's exit node.
pub const FUNCTION_EXIT_KIND: &str = "FunctionExit";
/// The node kind that calls a registered function.
pub const CALL_FUNCTION_KIND: &str = "CallFunction";

/// A single input or output pin of a function signature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FnPin {
    /// Pin name, matched against the body's entry/exit pin name.
    pub name: String,
    /// Data type carried by the pin.
    pub data_type: DataType,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Default value used when the caller leaves the argument unconnected.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Whether the argument may stay unconnected (resolves to null).
    #[serde(default)]
    pub optional: bool,
}

/// Function signature derived from the body's entry/exit nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSignature {
    /// Data output pins of the `FunctionEntry` node, in order.
    pub inputs: Vec<FnPin>,
    /// Data input pins of the `FunctionExit` node, in order.
    pub outputs: Vec<FnPin>,
}

/// The origin of a registered function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionSource {
    /// Read-only contribution of a verified addon package.
    Addon,
    /// Shipped with the daemon.
    Builtin,
    /// Stored in the global database.
    Global,
    /// Stored in a workspace database.
    Workspace,
}

/// A callable blueprint function registered in the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionEntry {
    /// Stable identifier (also used by function frames during resume).
    pub id: Uuid,
    /// PascalCase name, following the tool naming convention.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// The call signature.
    pub signature: FunctionSignature,
    /// The function body whose entry node is `FunctionEntry` and whose exit
    /// node is `FunctionExit`.
    pub body: Blueprint,
    /// Where this function was registered from.
    pub source: FunctionSource,
}

impl FunctionEntry {
    /// Derives the signature from `body`'s entry/exit node pins.
    ///
    /// Errors when the body lacks the required entry or exit node.
    pub fn derive_signature(body: &Blueprint) -> Result<FunctionSignature, String> {
        let entry = body
            .nodes
            .iter()
            .find(|n| n.kind == FUNCTION_ENTRY_KIND)
            .ok_or_else(|| format!("function body missing a '{FUNCTION_ENTRY_KIND}' node"))?;
        let exit = body
            .nodes
            .iter()
            .find(|n| n.kind == FUNCTION_EXIT_KIND)
            .ok_or_else(|| format!("function body missing a '{FUNCTION_EXIT_KIND}' node"))?;
        let pins = |node: &Node, pin_type: PinType| {
            node.pins
                .iter()
                .filter(|p| p.pin_type == pin_type)
                .map(|p| FnPin {
                    name: p.name.clone(),
                    data_type: p.data_type.clone(),
                    description: p.description.clone(),
                    default: p.default.clone(),
                    optional: p.optional,
                })
                .collect()
        };
        Ok(FunctionSignature {
            inputs: pins(entry, PinType::DataOutput),
            outputs: pins(exit, PinType::DataInput),
        })
    }
}
