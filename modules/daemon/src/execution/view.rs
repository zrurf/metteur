//! Read-only execution facts persisted in the existing checkpoint.
use super::ExecutionEvent;
use crate::storage::versioning::VersionRef;
use metteur_shared::{Blueprint, NodeId, PinId, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionView {
    pub root: Option<Blueprint>,
    pub graphs: HashMap<String, Blueprint>,
    pub invocations: Vec<Invocation>,
    pub edges: Vec<Traversal>,
    pub sequence: u64,
    #[serde(default)]
    pub changes: Vec<super::blackboard::Change>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    pub sequence: u64,
    pub current: bool,
    pub node_id: NodeId,
    pub scope: String,
    pub frame: Vec<String>,
    pub attempt: u32,
    pub version: Option<VersionRef>,
    pub inputs: serde_json::Value,
    pub outputs: serde_json::Value,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub status: String,
    pub messages: Vec<String>,
    #[serde(default)]
    pub check: Option<bool>,
    #[serde(default)]
    pub tree_id: Option<String>,
    #[serde(default)]
    pub owned_frame: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Traversal {
    pub edge_id: uuid::Uuid,
    pub scope: String,
    pub frame: Vec<String>,
    pub sequence: u64,
}
impl ExecutionView {
    pub fn begin(
        &mut self,
        graph: &Blueprint,
        node_id: NodeId,
        frame: Vec<String>,
        version: Option<VersionRef>,
        inputs: &HashMap<PinId, Value>,
        at: u64,
    ) {
        let scope = graph.id.to_string();
        if self
            .invocations
            .iter()
            .any(|i| i.current && i.node_id == node_id && i.scope == scope && i.frame == frame)
        {
            self.invalidate(&[node_id], &frame, super::blackboard::ChangeKind::Retry);
        }
        self.sequence += 1;
        self.graphs.insert(scope.clone(), graph.clone());
        let attempt = self
            .invocations
            .iter()
            .filter(|i| i.node_id == node_id && i.scope == scope && i.frame == frame)
            .count() as u32
            + 1;
        self.invocations.push(Invocation {
            current: true,
            sequence: self.sequence,
            node_id,
            scope,
            frame,
            attempt,
            version,
            inputs: values(inputs.iter()),
            outputs: serde_json::json!({}),
            started_at: at,
            finished_at: None,
            status: "Running".into(),
            messages: Vec::new(),
            check: None,
            tree_id: None,
            owned_frame: None,
        });
    }
    pub fn observe(&mut self, event: &ExecutionEvent, scope: &str, at: u64) {
        let node_id = match event {
            ExecutionEvent::NodeFinished {
                node_id,
            }
            | ExecutionEvent::NodeData {
                node_id,
                ..
            }
            | ExecutionEvent::Message {
                node_id,
                ..
            } => node_id,
            _ => return,
        };
        let Some(record) =
            self.invocations.iter_mut().rev().find(|i| &i.node_id == node_id && i.scope == scope)
        else {
            return;
        };
        match event {
            ExecutionEvent::NodeFinished {
                ..
            } => {
                record.status = "Completed".into();
                record.finished_at = Some(at);
            }
            ExecutionEvent::NodeData {
                outputs,
                ..
            } => {
                record.outputs = values(outputs.iter().map(|item| (&item.0, &item.1)));
            }
            ExecutionEvent::Message {
                message,
                ..
            } => {
                record.messages.push(message.clone());
            }
            _ => {}
        }
    }
    pub fn traverse(&mut self, graph: &Blueprint, edge_id: uuid::Uuid, frame: Vec<String>) {
        let Some(edge) = graph.edges.iter().find(|edge| edge.id == edge_id) else {
            return;
        };
        let Some(source) = self.invocations.iter().rev().find(|i| {
            i.node_id == edge.source_node
                && i.scope == graph.id.to_string()
                && i.frame == frame
                && i.current
        }) else {
            return;
        };
        self.edges.push(Traversal {
            edge_id,
            scope: graph.id.to_string(),
            frame,
            sequence: source.sequence,
        });
    }
    pub fn invalidate(
        &mut self,
        nodes: &[NodeId],
        frame: &[String],
        reason: super::blackboard::ChangeKind,
    ) {
        let children: Vec<_> = self
            .invocations
            .iter()
            .filter(|i| nodes.contains(&i.node_id) && i.frame == frame)
            .filter_map(|i| i.owned_frame.clone())
            .collect();
        let mut invalidates = Vec::new();
        for record in &mut self.invocations {
            if record.current
                && ((nodes.contains(&record.node_id) && record.frame == frame)
                    || children.iter().any(|prefix| record.frame.starts_with(prefix)))
            {
                record.current = false;
                invalidates.push(record.sequence);
            }
        }
        self.record_change(reason, invalidates, None, None);
    }
    pub(crate) fn record_change(
        &mut self,
        kind: super::blackboard::ChangeKind,
        invalidates: Vec<u64>,
        version: Option<VersionRef>,
        evidence: Option<String>,
    ) {
        self.sequence += 1;
        self.changes.push(super::blackboard::Change {
            sequence: self.sequence,
            kind,
            invalidates,
            version,
            evidence,
        });
    }
    pub(crate) fn invalidate_all(&mut self, reason: super::blackboard::ChangeKind) {
        let invalidates = self
            .invocations
            .iter_mut()
            .filter(|i| i.current)
            .map(|i| {
                i.current = false;
                i.sequence
            })
            .collect();
        self.record_change(reason, invalidates, None, None);
    }
    pub fn end(&mut self, status: &str, error: Option<&str>, at: u64) {
        for record in self.invocations.iter_mut().filter(|r| r.finished_at.is_none()) {
            record.status = status.into();
            record.finished_at = Some(at);
            if let Some(error) = error {
                record.messages.push(error.into());
            }
        }
    }
}
fn values<'a>(items: impl Iterator<Item = (&'a PinId, &'a Value)>) -> serde_json::Value {
    serde_json::Value::Object(
        items.map(|(id, value)| (id.to_string(), super::nodes::value_to_json(value))).collect(),
    )
}
