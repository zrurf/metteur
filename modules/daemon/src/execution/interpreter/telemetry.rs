//! Execution-tree bookkeeping for nodes entering and leaving the run.

use metteur_shared::Node;

use crate::execution::context::ExecutionContext;
use crate::execution::tree::{TreeNodeKind, TreeNodeStatus, TreeOp};

use super::Interpreter;
use super::checkpointing::now_millis;

impl Interpreter {
    /// Records the start of a node execution in the execution tree.
    pub(crate) fn tree_begin(&mut self, node: &Node) {
        let parent = self.frame_trees.last().map(String::as_str).or(self.tree_root.as_deref());
        let id = self.tree.spawn(
            parent,
            TreeNodeKind::BlueprintNode(node.kind.clone()),
            node.kind.clone(),
            now_millis(),
        );
        if let Some(record) = self.view.invocations.last_mut() {
            record.tree_id = Some(id.clone());
        }
        self.current_tree = Some(id);
    }

    /// Drains executor-queued tree ops, then marks the node finished.
    pub(crate) fn tree_end(&mut self, ctx: &mut ExecutionContext, status: TreeNodeStatus) {
        let ops = std::mem::take(&mut ctx.tree_ops);
        for op in ops {
            match op {
                TreeOp::SpawnChild {
                    kind,
                    label,
                } => {
                    let parent = self.current_tree.as_deref();
                    let id = self.tree.spawn(parent, kind, label, now_millis());
                    self.current_tree = Some(id);
                }
                TreeOp::FinishCurrent {
                    status,
                } => {
                    if let Some(id) = self.current_tree.clone() {
                        self.tree.finish(&id, status, now_millis());
                        self.current_tree = self.tree.nodes.get(&id).and_then(|n| n.parent.clone());
                    }
                }
                TreeOp::AddTokens(n) => {
                    if let Some(id) = &self.current_tree {
                        self.tree.add_tokens(id, n);
                    }
                }
            }
        }
        if let Some(id) = self.current_tree.clone() {
            self.tree.finish(&id, status, now_millis());
            self.current_tree = self.tree.nodes.get(&id).and_then(|n| n.parent.clone());
        }
    }
}
