use super::types::*;
use crate::{Blueprint, SharedError, SharedResult};
use std::collections::HashSet;

fn invalid(message: &str) -> SharedError {
    SharedError::Invalid(message.into())
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.contains('\0')
}
fn id(value: &str) -> bool {
    text(value, 64) && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn data_input<'a>(node: &'a crate::Node, name: &str) -> Option<&'a serde_json::Value> {
    let pin =
        node.pins.iter().find(|p| p.name == name && p.pin_type == crate::PinType::DataInput)?;
    node.data
        .get(name)
        .or_else(|| pin.key.as_ref().and_then(|key| node.data.get(key)))
        .or_else(|| node.data.get(pin.id.to_string()))
}

/// Portable workspace-relative paths. The host must additionally resolve links.
pub fn validate_path(path: &str) -> SharedResult<()> {
    if !text(path, 512)
        || path.contains(['\\', ':', '*', '?'])
        || path.starts_with('/')
        || path.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || p.eq_ignore_ascii_case(".metteur")
                || p.eq_ignore_ascii_case(".git")
                || p.ends_with(['.', ' '])
        })
    {
        return Err(invalid(
            "Use a workspace-relative file path without metadata, traversal or wildcards",
        ));
    }
    Ok(())
}
fn paths(paths: &[String]) -> SharedResult<()> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(invalid("Specify 1 to 64 workspace file paths"));
    }
    let mut seen = HashSet::new();
    for path in paths {
        validate_path(path)?;
        if !seen.insert(path) {
            return Err(invalid("Duplicate workspace path"));
        }
    }
    Ok(())
}
fn command(check: &CommandCheck) -> bool {
    id(&check.id)
        && text(&check.description, 1024)
        && text(&check.command, 4096)
        && (0..=255).contains(&check.expected_exit_code)
}

pub fn validate_input(input: &TemplateInput) -> SharedResult<()> {
    if input.instance_id.is_nil()
        || !text(&input.name, 120)
        || !text(&input.goal, 4096)
        || input.constraints.len() > 4096
        || input.constraints.contains('\0')
        || !text(&input.model, 200)
        || !(1..=64).contains(&input.max_iterations)
    {
        return Err(invalid(
            "Specify an instance ID, name, goal, configured model and 1 to 64 iterations",
        ));
    }
    paths(&input.scope_paths)?;
    paths(&input.evidence_paths)?;
    if !command(&input.baseline)
        || !input.baseline.required
        || input.checks.is_empty()
        || input.checks.len() + input.manual_checks.len() + 1 > MAX_CRITERIA
    {
        return Err(invalid(
            "A required baseline and final check are needed; at most 16 criteria are supported",
        ));
    }
    if (input.template == TemplateKind::FixBug && input.baseline.expected_exit_code == 0)
        || (input.template != TemplateKind::FixBug && input.baseline.expected_exit_code != 0)
    {
        return Err(invalid(
            "Bug reproduction must expect a nonzero exit code; other baselines must expect zero",
        ));
    }
    let mut seen = HashSet::from([input.baseline.id.as_str()]);
    for check in &input.checks {
        if !command(check) || check.expected_exit_code != 0 || !seen.insert(&check.id) {
            return Err(invalid(
                "Final checks need unique IDs, descriptions, commands and expected exit code zero",
            ));
        }
    }
    if !input.checks.iter().any(|c| c.required) {
        return Err(invalid("At least one final check must be required"));
    }
    for check in &input.manual_checks {
        if !id(&check.id) || !text(&check.description, 1024) || !seen.insert(&check.id) {
            return Err(invalid("Manual criteria need unique IDs and descriptions"));
        }
    }
    Ok(())
}

/// Read only the contract carried by the authoritative graph, never a template's latest version.
pub fn read_contract(graph: &Blueprint) -> SharedResult<Option<WorkflowContract>> {
    let marked: Vec<_> =
        graph.nodes.iter().filter(|n| n.data.get(CONTRACT_KEY).is_some()).collect();
    if marked.is_empty() {
        return Ok(None);
    }
    if marked.len() != 1 || marked[0].id != graph.entry_node_id || marked[0].kind != "Start" {
        return Err(invalid("Workflow metadata must appear once, on the Start entry node"));
    }
    let raw = &marked[0].data[CONTRACT_KEY];
    if raw.to_string().len() > 32 * 1024 {
        return Err(invalid("Workflow contract exceeds 32 KiB"));
    }
    let contract: WorkflowContract = serde_json::from_value(raw.clone())
        .map_err(|_| invalid("Invalid workflow contract fields"))?;
    if contract.schema_version != SCHEMA_VERSION || contract.template_revision != TEMPLATE_REVISION
    {
        return Err(invalid("Unsupported workflow contract or template revision"));
    }
    if contract.instance_id != graph.id
        || !text(&contract.goal, 4096)
        || contract.constraints.len() > 4096
        || contract.criteria.is_empty()
        || contract.criteria.len() > MAX_CRITERIA
        || contract.stages.len() > 32
    {
        return Err(invalid("Invalid workflow identity or limits"));
    }
    paths(&contract.scope_paths)?;
    paths(&contract.evidence_paths)?;
    let mut seen = HashSet::new();
    if contract.criteria.iter().filter(|c| c.phase == CriterionPhase::Baseline).count() != 1
        || !contract.criteria.iter().any(|c| {
            c.phase == CriterionPhase::Acceptance
                && c.required
                && matches!(
                    c.check,
                    CheckBinding::Command {
                        expected_exit_code: 0,
                        ..
                    }
                )
        })
    {
        return Err(invalid("Keep one required baseline and at least one required final check"));
    }
    for criterion in &contract.criteria {
        if !id(&criterion.id) || !text(&criterion.description, 1024) || !seen.insert(&criterion.id)
        {
            return Err(invalid("Invalid or duplicate workflow criterion"));
        }
        if let CheckBinding::Command {
            command_node,
            validator_node,
            command,
            expected_exit_code,
        } = &criterion.check
        {
            if criterion.phase == CriterionPhase::Baseline
                && (!criterion.required
                    || (contract.template == TemplateKind::FixBug) == (*expected_exit_code == 0))
                || criterion.phase == CriterionPhase::Acceptance && *expected_exit_code != 0
            {
                return Err(invalid(
                    "The baseline and final exits must preserve the template acceptance meaning",
                ));
            }
            let cmd = graph
                .node(*command_node)
                .ok_or_else(|| invalid("Workflow command node is missing"))?;
            let validator = graph
                .node(*validator_node)
                .ok_or_else(|| invalid("Workflow validator node is missing"))?;
            if !text(command, 4096)
                || !(0..=255).contains(expected_exit_code)
                || graph.edges.iter().any(|e| {
                    (e.target_node == *command_node
                        && cmd.pins.iter().any(|p| {
                            p.id == e.target_pin && p.pin_type == crate::PinType::DataInput
                        }))
                        || (e.target_node == *validator_node
                            && validator
                                .pins
                                .iter()
                                .any(|p| p.id == e.target_pin && p.name == "Expected"))
                })
            {
                return Err(invalid("Workflow commands and expected exits must be fixed literals"));
            }
            if !(cmd.kind == "Tool"
                && cmd.data.get("tool_name").and_then(|v| v.as_str()) == Some("ExecuteCommand"))
                || data_input(cmd, "command").and_then(|v| v.as_str()) != Some(command)
                || validator.kind != "Validator"
                || data_input(validator, "Expected").and_then(|v| v.as_i64())
                    != Some(i64::from(*expected_exit_code))
                || validator.data.get("mode").and_then(|v| v.as_str()) != Some("eq")
            {
                return Err(invalid(
                    "Workflow check configuration changed; update the acceptance contract explicitly",
                ));
            }
            // Require the complete command -> exit code -> validator data chain.
            let actual = validator
                .pins
                .iter()
                .find(|p| p.name == "Actual")
                .ok_or_else(|| invalid("Validator Actual pin missing"))?;
            let input = graph
                .edges
                .iter()
                .find(|e| e.target_node == *validator_node && e.target_pin == actual.id)
                .ok_or_else(|| {
                    invalid("Workflow validator must consume its command's exit code")
                })?;
            let extract = graph
                .node(input.source_node)
                .ok_or_else(|| invalid("Workflow exit-code extraction missing"))?;
            if !extract.pins.iter().any(|p| {
                p.id == input.source_pin
                    && p.name == "Result"
                    && p.pin_type == crate::PinType::DataOutput
            }) {
                return Err(invalid("Workflow validator must consume the extraction result"));
            }
            let object = extract
                .pins
                .iter()
                .find(|p| p.name == "Object")
                .ok_or_else(|| invalid("Workflow extraction Object pin missing"))?;
            if graph
                .edges
                .iter()
                .filter(|e| e.target_node == *validator_node && e.target_pin == actual.id)
                .count()
                != 1
                || graph
                    .edges
                    .iter()
                    .filter(|e| e.target_node == extract.id && e.target_pin == object.id)
                    .count()
                    != 1
                || graph.edges.iter().any(|e| {
                    e.target_node == extract.id
                        && extract.pins.iter().any(|p| p.id == e.target_pin && p.name == "Path")
                })
            {
                return Err(invalid(
                    "The command exit-code extraction must have one fixed source and path",
                ));
            }
            if extract.kind != "JsonGet"
                || data_input(extract, "Path").and_then(|v| v.as_str()) != Some("exit_code")
                || !graph.edges.iter().any(|e| {
                    e.target_node == extract.id
                        && e.target_pin == object.id
                        && e.source_node == *command_node
                        && cmd.pins.iter().any(|p| p.id == e.source_pin && p.name == "Result")
                })
            {
                return Err(invalid("Workflow check must read the recorded command exit code"));
            }
        } else if criterion.phase == CriterionPhase::Baseline {
            return Err(invalid("The baseline must be a deterministic command check"));
        }
    }
    for stage in &contract.stages {
        if !text(&stage.title, 120)
            || stage.nodes.is_empty()
            || stage.nodes.iter().any(|id| graph.node(*id).is_none())
        {
            return Err(invalid("Workflow stage references missing nodes"));
        }
    }
    Ok(Some(contract))
}
