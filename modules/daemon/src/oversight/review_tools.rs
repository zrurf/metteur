//! The supervisor authors the same data edits the application validates.
use metteur_shared::llm::ToolDefinition;
use serde_json::json;

pub(super) fn definition() -> ToolDefinition {
    let selector = json!({
        "type":"object",
        "description":"Copy edit_match from the target ReadBlueprint node. Only the current root is editable; a null edit_match is not an editable target.",
        "properties":{
            "kind":{"type":"string"},
            "nth":{"type":"integer","minimum":1,"description":"ONE-BASED occurrence among nodes of this kind in root blueprint array order. First is 1, second is 2. Do not count other scopes."}
        },
        "required":["kind","nth"],"additionalProperties":false
    });
    ToolDefinition {
        name: "ProposeBlueprintEdits".into(),
        description: "Propose one concrete change per review to unexecuted root nodes, or the server-bound circuit_node that will be retried before any failed successor. ReadBlueprint first and copy the target's edit_match. set_pin updates one exact node.data key (use the input pin's key, not its display name); set_data replaces the entire node.data value, so preserve other fields. Neither operation changes edges or pin definitions. The server may use existing scoped delegation for independent non-dangerous system actions in autonomous mode. Concierge-related actions and circuit retries always require separate per-proposal user confirmation; you cannot create or expand delegation. Confirmation only stages application at a safe boundary.".into(),
        parameters: json!({
            "type":"object",
            "properties":{
                "summary":{"type":"string"},
                "edits":{"type":"array","minItems":1,"items":{"oneOf":[
                    {"type":"object","properties":{
                        "op":{"type":"string","enum":["set_pin"]},
                        "match":selector,
                        "pin":{"type":"string","description":"Exact node.data key; prefer the target input pin's key, e.g. ms rather than Ms."},
                        "value":{}
                    },"required":["op","match","pin","value"],"additionalProperties":false},
                    {"type":"object","properties":{
                        "op":{"type":"string","enum":["set_data"]},
                        "match":selector,
                        "data":{"description":"Complete replacement for node.data, including unchanged fields."}
                    },"required":["op","match","data"],"additionalProperties":false}
                ]}}
            },
            "required":["summary","edits"],"additionalProperties":false
        }),
    }
}
