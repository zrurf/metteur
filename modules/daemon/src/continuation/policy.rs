//! Explicit bounded user policy; model text cannot create this record.
use super::state::{Budget, Task, TaskState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub detached: bool,
    pub max_turns: u32,
    pub token_limit: u64,
    pub expires_ms: u64,
    pub purpose: String,
}
impl Policy {
    pub fn validate(&self, now: u64) -> Result<(), &'static str> {
        if self.max_turns == 0
            || self.max_turns > 20
            || self.token_limit == 0
            || self.token_limit > 10_000_000
            || self.expires_ms <= now
            || self.expires_ms - now > 7 * 24 * 3600 * 1000
            || self.purpose.trim().is_empty()
            || self.purpose.len() > 2048
        {
            return Err("Invalid bounded continuation policy");
        }
        Ok(())
    }
    pub fn task(&self, session: Uuid, revision: u64) -> Task {
        Task {
            id: Uuid::new_v4(),
            session,
            generation: 1,
            expected_revision: revision,
            state: TaskState::Active,
            paused: false,
            policy_active: true,
            detached: self.detached,
            sources: BTreeMap::new(),
            events: BTreeMap::new(),
            wait: None,
            dispatch: None,
            dispatch_history: vec![],
            stop_generation: None,
            attention: None,
            budget: Budget {
                max_turns: self.max_turns,
                used_turns: 1,
                token_limit: self.token_limit,
                reserved_tokens: 0,
                consumed_tokens: 0,
                unknown_usage: false,
                call_ids: BTreeSet::new(),
            },
        }
    }
}

pub fn binding_digest(
    config: &metteur_shared::config::Config,
    model: Option<&str>,
    registry: &crate::Registry,
) -> String {
    let key = model.or(config.llm.default_model.as_deref());
    let mut model = key
        .and_then(|k| config.llm.models.get(k))
        .and_then(|m| serde_json::to_value(m).ok())
        .unwrap_or(serde_json::Value::Null);
    if let Some(o) = model.as_object_mut() {
        o.remove("api_key");
    }
    let mut tools: Vec<_> =
        registry.tools().into_iter().map(|t| (t.name().to_owned(), t.parameters())).collect();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    // The authorization fingerprint contains no credential bytes or user message text.
    crate::storage::resources::digest(serde_json::json!({"model_key":key,"model":model,"tools":tools,"harness_revision":"nx-m2-r1","permission_mode":metteur_shared::config::effective_permission_mode(&config.sandbox)}).to_string().as_bytes())
}
