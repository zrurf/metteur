//! Persisted overrides, separate from the fully populated runtime configuration.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::Config;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigLayer {
    /// Unmarked files retain the historical default-as-unset merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_version: Option<u32>,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl ConfigLayer {
    /// Older RPC clients serialize absent Options as null. TOML represents
    /// those as absent keys, exactly as the old typed serializer did.
    pub fn discard_legacy_nulls(&mut self) {
        fn clean(value: &mut Value) {
            if let Some(map) = value.as_object_mut() {
                map.retain(|_, v| !v.is_null());
                for v in map.values_mut() {
                    clean(v);
                }
            }
        }
        if self.config_version.is_none() {
            self.fields.retain(|_, v| !v.is_null());
            for value in self.fields.values_mut() {
                clean(value);
            }
        }
    }
    pub fn effective(&self) -> Result<Config, serde_json::Error> {
        if self.config_version.is_some_and(|v| v != 2) {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "unsupported config_version; expected 2",
            ));
        }
        validate(serde_json::from_value(Value::Object(self.fields.clone()))?)
    }

    /// Collections keep their established merge policy, including the global-only
    /// addon trust policy. Only section scalars acquire presence semantics.
    pub fn merge(&self, global: &Config) -> Result<Config, serde_json::Error> {
        let workspace = self.effective()?;
        let legacy = global.merge(&workspace);
        if self.config_version.is_none() {
            return Ok(legacy);
        }
        let mut merged = serde_json::to_value(legacy)?;
        let inherited = serde_json::to_value(global)?;
        let known = serde_json::to_value(Config::default())?;
        for (section, values) in inherited.as_object().unwrap() {
            if known.get(section).is_none() {
                continue;
            }
            if let Some(values) = values.as_object() {
                for (key, value) in values {
                    if value.is_array()
                        || value.is_object()
                        || (section == "addon" && key != "call_timeout_ms")
                    {
                        continue;
                    }
                    merged[section][key] =
                        self.fields.get(section).and_then(|s| s.get(key)).unwrap_or(value).clone();
                }
            }
        }
        // Preserve nested presence semantics and compatibility with existing files.
        if let Some(values) = self.fields.get("oversight").and_then(Value::as_object) {
            let mut section = inherited.get("oversight").cloned().unwrap_or(Value::Object(Map::new()));
            merge_presence(&mut section, &Value::Object(values.clone()));
            merged["oversight"] = section;
        }
        validate(serde_json::from_value(merged)?)
    }

    /// A proposed v2 layer with the same legacy semantics. Reading never writes
    /// it to disk: clients explicitly save/migrate after showing the notice.
    pub fn compatible_overrides(&self, workspace: bool) -> Result<Self, serde_json::Error> {
        let parsed = self.effective()?;
        if self.config_version == Some(2) {
            return Ok(self.clone());
        }
        if !workspace {
            return Ok(Self {
                config_version: Some(2),
                fields: self.fields.clone(),
            });
        }
        let defaults = serde_json::to_value(Config::default())?;
        let mut fields = serde_json::to_value(parsed)?.as_object().unwrap().clone();
        for (section, values) in &mut fields {
            if let Some(values) = values.as_object_mut() {
                if defaults.get(section).is_none() {
                    continue;
                }
                values.retain(|key, value| {
                    if section == "addon" && key != "call_timeout_ms" {
                        return false;
                    }
                    if (section == "llm" && key == "anonymize_thinking")
                        || (section == "lsp" && key == "check_on_node_end")
                    {
                        return true;
                    }
                    if section == "lsp" && key == "debounce_ms" {
                        return value != &Value::from(300);
                    }
                    if section == "sandbox" && key == "mode" {
                        return value.as_str().is_some_and(|s| !s.trim().is_empty());
                    }
                    if section == "llm" && key == "project_instruction_files" {
                        return value != &defaults[section][key];
                    }
                    if value.is_array() {
                        return !value.as_array().unwrap().is_empty();
                    }
                    if value.is_object() {
                        return !value.as_object().unwrap().is_empty();
                    }
                    if value.is_null() {
                        return false;
                    }
                    value != &defaults[section][key]
                });
            }
        }
        fields.retain(|_, v| !v.as_object().is_some_and(Map::is_empty));
        Ok(Self {
            config_version: Some(2),
            fields,
        })
    }
}

fn merge_presence(base: &mut Value, overrides: &Value) {
    if let (Some(base), Some(overrides)) = (base.as_object_mut(), overrides.as_object()) {
        for (key, value) in overrides {
            merge_presence(base.entry(key.clone()).or_insert(Value::Null), value);
        }
    } else {
        *base = overrides.clone();
    }
}

fn validate(config: Config) -> Result<Config, serde_json::Error> {
    super::oversight::OversightConfig::from_config(&config)?;
    for model in config.llm.models.values() {
        model.pricing.validate().map_err(<serde_json::Error as serde::de::Error>::custom)?;
    }
    Ok(config)
}
