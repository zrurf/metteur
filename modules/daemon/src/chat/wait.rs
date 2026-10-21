//! Deferred wait requests are data, never an authorization or a source receipt.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    Any,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitIntent {
    pub sources: Vec<String>,
    pub mode: MatchMode,
    pub deadline_ms: u64,
    pub params: serde_json::Value,
}
impl WaitIntent {
    pub fn validate(&self, now_ms: u64) -> Result<(), &'static str> {
        if self.sources.is_empty() || self.sources.len() > 16 {
            return Err("Wait requires 1 to 16 owned sources");
        }
        let unique: std::collections::HashSet<_> = self.sources.iter().collect();
        if unique.len() != self.sources.len()
            || self.sources.iter().any(|s| s.is_empty() || s.len() > 128)
        {
            return Err("Invalid wait source identity");
        }
        if self.deadline_ms <= now_ms {
            return Err("Wait deadline must be in the future");
        }
        if !self.params.is_object() || self.params.to_string().len() > 8192 {
            return Err("Wait parameters exceed the object limit");
        }
        Ok(())
    }
}
