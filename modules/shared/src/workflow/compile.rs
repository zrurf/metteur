use super::{read_contract, types::*, validate_input};
use crate::{Blueprint, SharedError, SharedResult, node_catalog::NodeCatalog};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use uuid::Uuid;

/// Compile without executing, persisting, selecting prompts or granting permissions.
pub fn compile_template(input: &TemplateInput, catalog: &NodeCatalog) -> SharedResult<Blueprint> {
    validate_input(input)?;
    let mut nodes = Map::from_iter([
        ("start".into(), json!({"kind":"Start"})),
        ("implement".into(), json!({"kind":"CallLLM"})),
        ("baseline_gate".into(), json!({"kind":"Branch","Condition":"$baseline_pass.Passed"})),
        ("end".into(), json!({"kind":"End"})),
    ]);
    let mut chain = vec!["start".to_owned()];
    let mut checks = vec![("baseline".to_owned(), &input.baseline)];
    checks.extend(input.checks.iter().enumerate().map(|(i, c)| (format!("check_{i}"), c)));
    for (index, (alias, check)) in checks.iter().enumerate() {
        if index == 1 {
            chain.extend(["baseline_gate".into(), "implement".into()]);
        }
        nodes.insert(alias.clone(), json!({"kind":"ExecuteCommand","cwd":".","timeout_secs":120}));
        nodes.insert(
            format!("{alias}_exit"),
            json!({"kind":"JsonGet","Object":format!("${alias}.Result"),"Path":"exit_code"}),
        );
        nodes.insert(format!("{alias}_pass"),json!({"kind":"Validator","Actual":format!("${alias}_exit.Result"),"Expected":check.expected_exit_code,"mode":"eq","retry":{"max_attempts":1,"rollback":false}}));
        chain.extend([alias.clone(), format!("{alias}_exit"), format!("{alias}_pass")]);
    }
    chain.push("end".into());
    for (alias, node) in &mut nodes {
        node["_workflow_alias"] = json!(alias);
    }
    let split = chain.iter().position(|s| s == "baseline_gate").unwrap();
    let draft = json!({"name":input.name,"nodes":nodes,"flow":[chain[..=split].join(" -> "),format!("baseline_gate.True -> {}",chain[split+1..].join(" -> ")),"baseline_gate.False -> end"]});
    let mut graph = crate::dsl::compile_draft_value_with_catalog(&draft, catalog)?;
    // Namespace every node/pin/edge by the instance identity. Same-input retries are stable,
    // while separate instances with identical names cannot alias each other's persisted graph.
    let mut ids = HashMap::new();
    let remap = |old: Uuid| Uuid::new_v5(&input.instance_id, old.as_bytes());
    for node in &mut graph.nodes {
        node.id = remap(node.id);
        let alias = node
            .data
            .as_object_mut()
            .unwrap()
            .remove("_workflow_alias")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        ids.insert(alias, node.id);
        node.pins.retain(|p| p.name != "_workflow_alias");
        for pin in &mut node.pins {
            pin.id = remap(pin.id);
        }
    }
    for edge in &mut graph.edges {
        edge.id = remap(edge.id);
        edge.source_node = remap(edge.source_node);
        edge.target_node = remap(edge.target_node);
        edge.source_pin = remap(edge.source_pin);
        edge.target_pin = remap(edge.target_pin);
    }
    graph.entry_node_id = remap(graph.entry_node_id);
    graph.id = input.instance_id;
    let node_id = |alias: &str| -> SharedResult<Uuid> {
        ids.get(alias)
            .copied()
            .ok_or_else(|| SharedError::Invalid("Template node identity missing".into()))
    };
    let implement = node_id("implement")?;
    let acceptance = templates().into_iter().find(|t| t.id == input.template).unwrap().acceptance;
    let prompt = format!(
        "Complete this bounded coding task.\nGoal: {}\nConstraints: {}\nAllowed task paths (not a permission grant): {}\nAcceptance: {}\nRequired checks: {}\nA separate deterministic stage runs the approved checks immediately after you return. Finish with a concise summary when the scoped edits and requested tests are ready; do not repeatedly run the acceptance commands in this implementation stage unless needed to diagnose a failure. Do not change checks merely to make them pass. Report unresolved requirements honestly. Keep changes focused; use existing tools and permissions.",
        input.goal,
        input.constraints,
        input.scope_paths.join(", "),
        acceptance,
        input
            .checks
            .iter()
            .map(|c| format!("{}: {}", c.id, c.description))
            .collect::<Vec<_>>()
            .join("; ")
    );
    graph.nodes.iter_mut().find(|n| n.id == implement).unwrap().data =
        json!({"prompt":prompt,"model":input.model,"max_iterations":input.max_iterations});
    let mut criteria = Vec::new();
    let mut stages = Vec::new();
    for (index, (alias, check)) in checks.iter().enumerate() {
        let command_node = node_id(alias)?;
        let validator_node = node_id(&format!("{alias}_pass"))?;
        // Assign user strings after compilation: "$..." in commands is literal input,
        // never a shorthand wire or a substitution language.
        graph.nodes.iter_mut().find(|n| n.id == command_node).unwrap().data["command"] =
            Value::String(check.command.clone());
        criteria.push(Criterion {
            id: check.id.clone(),
            description: check.description.clone(),
            required: check.required,
            phase: if index == 0 {
                CriterionPhase::Baseline
            } else {
                CriterionPhase::Acceptance
            },
            check: CheckBinding::Command {
                command_node,
                validator_node,
                command: check.command.clone(),
                expected_exit_code: check.expected_exit_code,
            },
        });
        if index == 1 {
            stages.push(Stage {
                title: "Implement the scoped change".into(),
                nodes: vec![implement],
            });
        }
        stages.push(Stage {
            title: if index == 0 {
                "Verify the baseline".into()
            } else {
                format!("Acceptance check {index}")
            },
            nodes: vec![command_node, node_id(&format!("{alias}_exit"))?, validator_node],
        });
    }
    criteria.extend(input.manual_checks.iter().map(|c| Criterion {
        id: c.id.clone(),
        description: c.description.clone(),
        required: c.required,
        phase: CriterionPhase::Acceptance,
        check: CheckBinding::Manual,
    }));
    let contract = WorkflowContract {
        schema_version: SCHEMA_VERSION,
        instance_id: input.instance_id,
        template: input.template,
        template_revision: TEMPLATE_REVISION,
        goal: input.goal.clone(),
        constraints: input.constraints.clone(),
        scope_paths: input.scope_paths.clone(),
        evidence_paths: input.evidence_paths.clone(),
        criteria,
        stages,
    };
    graph.nodes.iter_mut().find(|n| n.id == graph.entry_node_id).unwrap().data[CONTRACT_KEY] =
        serde_json::to_value(contract).map_err(|e| SharedError::Invalid(e.to_string()))?;
    read_contract(&graph)?;
    let report = crate::model::validate::validate_with_catalog(&graph, catalog);
    if !report.is_ok() {
        return Err(SharedError::Invalid(format!(
            "Template graph is invalid: {:?}",
            report.errors
        )));
    }
    Ok(graph)
}
