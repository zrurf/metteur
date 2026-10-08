use metteur_daemon::{
    oversight::budget::{self, Caller},
    storage::persistence::Db,
};
use metteur_shared::{
    Usage,
    config::{Config, ConfigLayer, oversight::OversightConfig},
};
use uuid::Uuid;

fn database() -> (std::path::PathBuf, Db) {
    let path = std::env::temp_dir().join(format!("oversight-budget-{}", Uuid::new_v4()));
    let db = Db::open(&path).unwrap();
    (path, db)
}
#[test]
fn explicit_zero_and_nested_presence_survive_config_merge() {
    let global: ConfigLayer = serde_json::from_value(serde_json::json!({"config_version":2,"oversight":{"run_token_budget":9000,"triggers":{"interval_ms":99,"on_validation_failed":true}}})).unwrap();
    let ws: ConfigLayer = serde_json::from_value(serde_json::json!({"config_version":2,"oversight":{"run_token_budget":0,"triggers":{"interval_ms":0}}})).unwrap();
    let settings =
        OversightConfig::from_config(&ws.merge(&global.effective().unwrap()).unwrap()).unwrap();
    assert_eq!(settings.run_token_budget, 0);
    assert_eq!(settings.triggers.interval_ms, 0);
    assert!(settings.triggers.on_validation_failed);
    assert!(settings.concierge_model.is_none());
    let (_, db) = database();
    assert!(budget::reserve(&db, Uuid::new_v4(), Caller::Concierge, "m", 1, &settings).is_err());
    for invalid in [
        serde_json::json!({"budget_warn_ratio":0}),
        serde_json::json!({"mode":"grant"}),
        serde_json::json!({"max_output_tokens":0}),
    ] {
        let layer: ConfigLayer =
            serde_json::from_value(serde_json::json!({"oversight":invalid})).unwrap();
        assert!(layer.effective().is_err());
    }
}
#[test]
fn concurrent_reservations_share_one_durable_limit_after_reopen() {
    let (path, db) = database();
    let run = Uuid::new_v4();
    let settings = OversightConfig {
        run_token_budget: 100,
        ..Default::default()
    };
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db = db.clone();
            let settings = settings.clone();
            std::thread::spawn(move || {
                budget::reserve(&db, run, Caller::Concierge, "m", 60, &settings).is_ok()
            })
        })
        .collect();
    assert_eq!(handles.into_iter().map(|h| u32::from(h.join().unwrap())).sum::<u32>(), 1);
    drop(db);
    let reopened = Db::open(&path).unwrap();
    assert_eq!(budget::summary(&reopened, run, &settings).unwrap().charged, 60);
    assert!(budget::reserve(&reopened, run, Caller::Supervisor, "strong", 60, &settings).is_err());
    let usage =
        metteur_daemon::llm::billing::run_usage(&reopened, &Default::default(), &run.to_string())
            .unwrap();
    assert!(!usage.models[0].tokens_complete);
    assert!(!usage.models[0].cost_complete);
}
#[test]
fn actual_usage_releases_unused_reservation_without_double_counting_cache() {
    let (_, db) = database();
    let run = Uuid::new_v4();
    let config = Config::default();
    let settings = OversightConfig::default();
    let id = budget::reserve(&db, run, Caller::Concierge, "m", 1000, &settings).unwrap();
    let usage = Usage {
        tokens_reported: true,
        cache_read_reported: true,
        input_tokens: 100,
        cached_input_tokens: 80,
        output_tokens: 10,
        reasoning_tokens: 5,
        ..Default::default()
    };
    budget::settle(&db, run, id, Some(usage), "m", &config).unwrap();
    assert_eq!(budget::summary(&db, run, &settings).unwrap().charged, 110);
    assert!(budget::settle(&db, run, id, Some(usage), "m", &config).is_err());
    let result =
        metteur_daemon::llm::billing::run_usage(&db, &config.billing, &run.to_string()).unwrap();
    assert_eq!(result.models[0].cached_input_tokens, 80);
    assert_eq!(result.models[0].input_tokens, 100);
}
#[tokio::test]
async fn timeout_keeps_reservation_and_does_not_fall_back() {
    let (_, db) = database();
    let run = Uuid::new_v4();
    let mut config = Config::default();
    config.extra.insert("oversight".into(), serde_json::json!({"concierge_timeout_ms":1}));
    let client = metteur_daemon::llm::MockClient::new_delayed(
        vec![metteur_daemon::llm::MockStep::Text("late".into())],
        std::time::Duration::from_millis(50),
    );
    let context = metteur_shared::llm::ContextManager::new_from_prompt(vec![], "status");
    assert!(
        budget::complete(&db, run, Caller::Concierge, "m", &config, &client, &context)
            .await
            .is_err()
    );
    let calls = budget::load(&db, run).unwrap().calls;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].state, "usage_unknown");
    assert_eq!(calls[0].charged, calls[0].reserved);
}

#[test]
fn explicit_delegation_revocation_and_invalid_scopes_fail_closed() {
    let node = uuid::Uuid::new_v4();
    let mut user = metteur_shared::config::Config::default();
    user.extra.insert("oversight".into(),serde_json::json!({"mode":"autonomous","delegation":{"pause_run":true,"blueprint_edits":[{"node_id":node,"fields":["prompt"]}]}}));
    let mut workspace = metteur_shared::config::Config::default();
    workspace.extra.insert(
        "oversight".into(),
        serde_json::json!({"delegation":{"pause_run":false,"blueprint_edits":[]}}),
    );
    let layer: ConfigLayer = serde_json::from_value(
        serde_json::json!({"config_version":2,"oversight":workspace.extra["oversight"]}),
    )
    .unwrap();
    let merged = layer.merge(&user).unwrap();
    let settings =
        metteur_shared::config::oversight::OversightConfig::from_config(&merged).unwrap();
    assert!(!settings.delegation.pause_run);
    assert!(settings.delegation.blueprint_edits.is_empty());
    assert_eq!(settings.mode, "autonomous");
    for grant in [
        serde_json::json!({"node_id":node,"fields":["*"]}),
        serde_json::json!({"node_id":node,"fields":[]}),
        serde_json::json!({"node_id":uuid::Uuid::nil(),"fields":["prompt"]}),
        serde_json::json!({"node_id":node,"fields":["prompt"],"approve_all":true}),
    ] {
        workspace.extra.insert(
            "oversight".into(),
            serde_json::json!({"delegation":{"blueprint_edits":[grant]}}),
        );
        assert!(
            metteur_shared::config::oversight::OversightConfig::from_config(&workspace).is_err()
        );
    }
}
