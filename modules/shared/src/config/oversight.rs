//! Validated oversight settings, using the existing presence-aware extension layer.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OversightConfig {
    pub mode: String,
    pub model: Option<String>,
    pub concierge_model: Option<String>,
    pub temperature: f64,
    pub max_review_iterations: u32,
    pub review_timeout_ms: u64,
    pub concierge_timeout_ms: u64,
    pub max_output_tokens: u32,
    pub run_token_budget: u64,
    pub budget_warn_ratio: f64,
    pub cooldown_ms: u64,
    pub triggers: Triggers,
    pub blackboard: BlackboardConfig,
    pub structure_edit_limit: u32,
    pub delegation: Delegation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Triggers {
    pub on_validation_failed: bool,
    pub on_node_finished: NodeTrigger,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeTrigger {
    Enabled(bool),
    Nodes(Vec<String>),
}
impl Default for NodeTrigger {
    fn default() -> Self {
        Self::Enabled(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlackboardConfig {
    pub max_entries: usize,
    pub digest_chars: usize,
}
impl Default for BlackboardConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            digest_chars: 2000,
        }
    }
}
impl Default for OversightConfig {
    fn default() -> Self {
        Self {
            mode: "assisted".into(),
            model: None,
            concierge_model: None,
            temperature: 0.2,
            max_review_iterations: 4,
            review_timeout_ms: 120_000,
            concierge_timeout_ms: 15_000,
            max_output_tokens: 2048,
            run_token_budget: 65536,
            budget_warn_ratio: 0.8,
            cooldown_ms: 30_000,
            triggers: Triggers::default(),
            blackboard: BlackboardConfig::default(),
            structure_edit_limit: 5,
            delegation: Delegation::default(),
        }
    }
}
impl OversightConfig {
    pub fn from_config(config: &super::Config) -> Result<Self, serde_json::Error> {
        let value: Self = serde_json::from_value(
            config.extra.get("oversight").cloned().unwrap_or_else(|| serde_json::json!({})),
        )?;
        if !matches!(value.mode.as_str(), "off" | "assisted" | "autonomous")
            || !value.temperature.is_finite()
            || !(0.0..=2.0).contains(&value.temperature)
            || !value.budget_warn_ratio.is_finite()
            || !(0.0 < value.budget_warn_ratio && value.budget_warn_ratio <= 1.0)
            || value.max_review_iterations == 0
            || value.review_timeout_ms == 0
            || value.concierge_timeout_ms == 0
            || value.max_output_tokens == 0
            || value.blackboard.max_entries == 0
            || value.blackboard.digest_chars == 0
            || value.delegation.blueprint_edits.iter().any(|g| {
                g.node_id.is_nil()
                    || g.fields.is_empty()
                    || g.fields.iter().any(|f| f.trim().is_empty() || f == "*")
            })
        {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "invalid oversight settings",
            ));
        }
        Ok(value)
    }
}

/// Explicit user configuration. Empty by default; models cannot create grants.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Delegation {
    pub pause_run: bool,
    pub blueprint_edits: Vec<EditDelegation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditDelegation {
    pub node_id: uuid::Uuid,
    pub fields: Vec<String>,
}
