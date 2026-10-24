use metteur_shared::{node_catalog::builtin_catalog, workflow::*};
use serde_json::json;
use uuid::Uuid;

fn input(kind: TemplateKind) -> TemplateInput {
    TemplateInput {
        instance_id: Uuid::new_v4(),
        template: kind,
        name: "Example change".into(),
        goal: "Handle empty input without changing valid input".into(),
        constraints: "Preserve public signatures".into(),
        scope_paths: vec!["src/normalize.py".into()],
        evidence_paths: vec!["src/normalize.py".into(), "tests/test_normalize.py".into()],
        model: "configured-model".into(),
        max_iterations: 8,
        baseline: CommandCheck {
            id: "baseline".into(),
            description: "Confirm the frozen baseline".into(),
            command: "python -m unittest".into(),
            expected_exit_code: if kind == TemplateKind::FixBug {
                1
            } else {
                0
            },
            required: true,
        },
        checks: vec![CommandCheck {
            id: "regression".into(),
            description: "The independent regression passes".into(),
            command: "python -m unittest".into(),
            expected_exit_code: 0,
            required: true,
        }],
        manual_checks: vec![ManualCheck {
            id: "scope".into(),
            description: "Review the scoped changes".into(),
            required: true,
        }],
    }
}

#[test]
fn all_templates_are_editable_ordinary_graphs_with_bound_checks() {
    for info in templates() {
        let parameters = input(info.id);
        let graph = compile_template(&parameters, &builtin_catalog()).unwrap();
        assert!(metteur_shared::validate(&graph).is_ok());
        let roundtrip = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        assert_eq!(graph, roundtrip);
        let contract = read_contract(&graph).unwrap().unwrap();
        assert_eq!(contract.criteria.len(), 3);
        assert_eq!(contract.stages.len(), 3);
        assert_eq!(contract.instance_id, parameters.instance_id);
        assert!(graph.nodes.iter().all(|n| n.data.get("_workflow_alias").is_none()));
        assert_eq!(graph.nodes.iter().filter(|n| n.kind == "CallLLM").count(), 1);
    }
}

#[test]
fn retry_is_stable_and_same_named_instances_are_isolated() {
    let mut parameters = input(TemplateKind::Refactor);
    let first = compile_template(&parameters, &builtin_catalog()).unwrap();
    assert_eq!(first, compile_template(&parameters, &builtin_catalog()).unwrap());
    parameters.instance_id = Uuid::new_v4();
    let other = compile_template(&parameters, &builtin_catalog()).unwrap();
    assert_ne!(first.id, other.id);
    assert!(first.nodes.iter().all(|n| other.node(n.id).is_none()));
    assert!(first.nodes.iter().flat_map(|n| &n.pins).all(|p| other.pin(p.id).is_none()));
}

#[test]
fn user_strings_are_literals_and_do_not_become_compiler_wires() {
    let mut parameters = input(TemplateKind::AddFeature);
    parameters.checks[0].command = "$env:CHECK='literal'; test-command".into();
    parameters.goal = "$start.Result must remain literal user text".into();
    let graph = compile_template(&parameters, &builtin_catalog()).unwrap();
    let contract = read_contract(&graph).unwrap().unwrap();
    if let CheckBinding::Command {
        command_node,
        ..
    } = contract.criteria[1].check
    {
        assert_eq!(graph.node(command_node).unwrap().data["command"], parameters.checks[0].command);
    }
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| n.data["prompt"].as_str().is_some_and(|p| p.contains(&parameters.goal)))
    );
}

#[test]
fn strict_input_and_portable_paths_reject_ambiguous_or_unsafe_templates() {
    for path in [
        "../outside",
        "C:/absolute",
        "/root/file",
        "a\\b",
        ".metteur/db",
        "src/../x",
        ".git/config",
        "src/*",
        "src/file.",
    ] {
        assert!(validate_path(path).is_err(), "{path}");
    }
    for path in ["src/main.rs", "tests/test a.py", "新增文件.py"] {
        assert!(validate_path(path).is_ok(), "{path}");
    }
    let p = input(TemplateKind::FixBug);
    for (field, value) in [
        ("instance_id", json!(Uuid::nil())),
        ("model", json!("")),
        ("max_iterations", json!(0)),
        ("scope_paths", json!([])),
        ("checks", json!([])),
    ] {
        let mut value_input = serde_json::to_value(&p).unwrap();
        value_input[field] = value;
        let changed: TemplateInput = serde_json::from_value(value_input).unwrap();
        assert!(compile_template(&changed, &builtin_catalog()).is_err(), "{field}");
    }
    let mut raw = serde_json::to_value(&p).unwrap();
    raw["approved"] = json!(true);
    assert!(serde_json::from_value::<TemplateInput>(raw).is_err());
}

#[test]
fn baseline_expectations_and_unique_criteria_are_enforced() {
    let mut p = input(TemplateKind::FixBug);
    p.baseline.expected_exit_code = 0;
    assert!(validate_input(&p).is_err());
    p = input(TemplateKind::AddTests);
    p.baseline.expected_exit_code = 1;
    assert!(validate_input(&p).is_err());
    p = input(TemplateKind::Refactor);
    p.checks[0].id = "baseline".into();
    assert!(validate_input(&p).is_err());
    p = input(TemplateKind::AddFeature);
    p.manual_checks[0].id = p.checks[0].id.clone();
    assert!(validate_input(&p).is_err());
    p = input(TemplateKind::AddFeature);
    p.checks[0].required = false;
    assert!(validate_input(&p).is_err());
}

#[test]
fn edited_commands_checks_and_future_schema_cannot_reuse_old_contracts() {
    let graph = compile_template(&input(TemplateKind::FixBug), &builtin_catalog()).unwrap();
    let contract = read_contract(&graph).unwrap().unwrap();
    let CheckBinding::Command {
        command_node,
        validator_node,
        ..
    } = contract.criteria[1].check
    else {
        panic!()
    };
    let mut changed = graph.clone();
    changed.nodes.iter_mut().find(|n| n.id == command_node).unwrap().data["command"] =
        json!("echo pass");
    assert!(!metteur_shared::validate(&changed).is_ok());
    changed = graph.clone();
    changed.nodes.iter_mut().find(|n| n.id == validator_node).unwrap().data["Expected"] = json!(1);
    assert!(read_contract(&changed).is_err());
    changed = graph.clone();
    changed.edges.retain(|e| e.target_node != validator_node);
    assert!(read_contract(&changed).is_err());
    changed = graph.clone();
    changed.nodes.iter_mut().find(|n| n.id == graph.entry_node_id).unwrap().data[CONTRACT_KEY]["schema_version"] =
        json!(99);
    assert!(read_contract(&changed).is_err());
}

#[test]
fn legacy_graphs_keep_their_existing_validation_behavior() {
    let legacy=metteur_shared::dsl::compile_draft_value(&json!({"name":"Legacy","nodes":{"start":{"kind":"Start"},"end":{"kind":"End"}},"flow":["start -> end"]})).unwrap();
    assert!(read_contract(&legacy).unwrap().is_none());
    assert!(metteur_shared::validate(&legacy).is_ok());
}
