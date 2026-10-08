use metteur_shared::config::{Config, ConfigLayer};
use serde_json::json;

fn layer(value: serde_json::Value) -> ConfigLayer {
    serde_json::from_value(value).unwrap()
}

#[test]
fn explicit_values_reset_and_roundtrip() {
    let global = layer(json!({"sandbox":{"enabled":true},"llm":{"thinking_budget_tokens":4096,"default_model":"global"},"oversight":{"mode":"assisted","run_token_budget":10}})).effective().unwrap();
    let mut ws = layer(
        json!({"config_version":2,"sandbox":{"enabled":false},"llm":{"thinking_budget_tokens":0,"default_model":""},"oversight":{"mode":"off","run_token_budget":0}}),
    );
    let merged = ws.merge(&global).unwrap();
    assert!(!merged.sandbox.enabled);
    assert_eq!(merged.llm.thinking_budget_tokens, 0);
    assert_eq!(merged.llm.default_model.as_deref(), Some(""));
    assert_eq!(merged.extra["oversight"], json!({"mode":"off","run_token_budget":0}));
    let saved = toml::to_string(&ws).unwrap();
    let reloaded: ConfigLayer = toml::from_str(&saved).unwrap();
    assert_eq!(reloaded.merge(&global).unwrap(), merged);
    ws.fields.get_mut("llm").unwrap().as_object_mut().unwrap().remove("thinking_budget_tokens");
    assert_eq!(ws.merge(&global).unwrap().llm.thinking_budget_tokens, 4096);
    assert_eq!(layer(json!({"config_version":2})).merge(&global).unwrap(), global);
}

#[test]
fn legacy_automatic_defaults_preserve_permissions_models_and_budgets() {
    let global = layer(json!({"sandbox":{"enabled":true,"mode":"ask"},"llm":{"thinking_budget_tokens":8192,"prompt_cache":false,"default_model":"global"},"execution":{"foreach_max_iterations":40}})).effective().unwrap();
    let old = layer(serde_json::to_value(Config::default()).unwrap());
    let before = global.merge(&old.effective().unwrap());
    assert_eq!(old.merge(&global).unwrap(), before);
    assert!(before.sandbox.enabled);
    assert_eq!(before.sandbox.mode, "ask");
    assert_eq!(before.llm.thinking_budget_tokens, 8192);
    assert!(!before.llm.prompt_cache);
    let migrated = old.compatible_overrides(true).unwrap();
    assert_eq!(migrated.merge(&global).unwrap(), before);
    // Reads and migration proposals don't alter the original layer.
    assert_eq!(old.config_version, None);
}

#[test]
fn migration_covers_every_field_and_collection_policy() {
    let global = layer(json!({"lsp":{"enabled":true,"debounce_ms":777},"addon":{"require_signature":true},"llm":{"project_instruction_files":["CUSTOM.md"]}})).effective().unwrap();
    for raw in [
        json!({}),
        json!({"lsp":{}}),
        json!({"llm":{"anonymize_thinking":true,"project_instruction_files":[]},"lsp":{"check_on_node_end":true},"addon":{"require_signature":false}}),
    ] {
        let old = layer(raw);
        assert_eq!(
            old.merge(&global).unwrap(),
            old.compatible_overrides(true).unwrap().merge(&global).unwrap()
        );
    }
    let ws = layer(
        json!({"config_version":2,"addon":{"require_signature":false,"signing_keys":["untrusted"]},"llm":{"models":{},"project_instruction_files":["METTEUR.md","AGENTS.md"]}}),
    );
    let merged = ws.merge(&global).unwrap();
    assert!(merged.addon.require_signature);
    assert!(merged.addon.signing_keys.is_empty());
    assert_eq!(merged.llm.project_instruction_files, vec!["CUSTOM.md"]);
    assert!(layer(json!({"config_version":3})).effective().is_err());
}

#[test]
fn nested_oversight_scalars_inherit_without_replacing_siblings() {
    let global = layer(json!({"oversight":{"triggers":{"interval_ms":500,"on_validation_failed":true},"blackboard":{"max_entries":100}}})).effective().unwrap();
    let ws = layer(json!({"config_version":2,"oversight":{"triggers":{"interval_ms":0}}}));
    assert_eq!(ws.merge(&global).unwrap().extra["oversight"], json!({"triggers":{"interval_ms":0,"on_validation_failed":true},"blackboard":{"max_entries":100}}));
}
