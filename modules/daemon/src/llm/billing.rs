//! Model pricing and run usage aggregation.

use std::collections::HashMap;

use metteur_shared::config::{BillingConfig, LlmModelConfig, ModelPricing};

use crate::error::DaemonResult;
use crate::llm::peak::{self, CronSchedule};

/// The computed cost of a single LLM call.
#[derive(Debug, Clone, PartialEq)]
pub struct Cost {
    /// Cost in millionths of the configured currency.
    pub micros: u64,
    /// Currency code from the configuration.
    pub currency: String,
}

/// Per-model aggregated usage for one execution run.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsageSummary {
    pub model: String,
    pub tokens_complete: bool,
    pub cache_complete: bool,
    pub cost_complete: bool,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub cost_micros: u64,
}

/// Aggregated usage and cost for one execution run.
#[derive(Debug, Clone, Default)]
pub struct UsageSummaryData {
    pub currency: String,
    pub total_cost_micros: u64,
    pub models: Vec<ModelUsageSummary>,
}

/// Computes the cost of one LLM call.
///
/// `pricing` is the model's effective price table (see [`effective_pricing`]);
/// `None` means the model has no configured pricing. Output already includes
/// reasoning. When the provider reports cache traffic, the
/// matching cache rates apply and fall back to the regular input rate when the
/// model config leaves them unset.
pub fn cost(
    pricing: Option<&ModelPricing>,
    currency: &str,
    usage: &metteur_shared::Usage,
) -> Option<Cost> {
    let p = pricing?;
    if !usage.tokens_reported || p.validate().is_err() {
        return None;
    }
    let uncached = usage.uncached_input_tokens();
    let cache_hit_rate =
        if p.cache_hit_per_mtok > 0.0 { p.cache_hit_per_mtok } else { p.input_per_mtok };
    let cache_write_rate =
        if p.cache_write_per_mtok > 0.0 { p.cache_write_per_mtok } else { p.input_per_mtok };
    let input = p.input_per_mtok * uncached as f64
        + cache_hit_rate * usage.cached_input_tokens as f64
        + cache_write_rate * usage.cache_write_input_tokens as f64;
    let output = p.output_per_mtok * usage.output_tokens as f64;
    let total = (input + output) / 1_000_000.0;

    Some(Cost {
        micros: (total * 1_000_000.0).round() as u64,
        currency: if currency.is_empty() {
            "USD".to_string()
        } else {
            currency.to_string()
        },
    })
}

/// Best-effort price table for a model, honouring its pricing `kind`.
///
/// `default` → flat `prices`; `tiered` → the first inclusive input bound; `peak` →
/// the first active window's prices, or `default_prices` outside all windows.
/// Windows are evaluated in `timezone` (defaults to UTC on unknown names).
pub fn effective_pricing(model: &LlmModelConfig, input_tokens: u64) -> Option<ModelPricing> {
    effective_pricing_at(model, input_tokens, "", chrono::Utc::now())
}

/// Time-aware variant of [`effective_pricing`].
pub fn effective_pricing_at(
    model: &LlmModelConfig,
    input_tokens: u64,
    timezone: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<ModelPricing> {
    model.pricing.validate().ok()?;
    match model.pricing.kind.as_str() {
        "tiered" => model.pricing.tiers.iter()
            .find(|tier| tier.max_input_tokens.is_none_or(|bound| input_tokens <= bound))
            .map(|tier| tier.prices.clone()),
        "peak" => peak_pricing(model, timezone, now),
        _ => model.pricing.prices.clone().or_else(|| model.pricing.default_prices.clone()),
    }
}

/// Resolves `peak` pricing: first active window wins, else `default_prices`.
///
/// The reporting time zone selects the local time windows are evaluated in
/// (unknown names fall back to UTC). Unparseable cron expressions are skipped
/// with a warning.
fn peak_pricing(
    model: &LlmModelConfig,
    timezone: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<ModelPricing> {
    let tz: chrono_tz::Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz);
    for window in &model.pricing.windows {
        match CronSchedule::parse(&window.cron) {
            Ok(schedule) if peak::window_active(&schedule, window.duration_min, &local) => {
                return Some(window.prices.clone());
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!("skipping invalid peak window cron '{}': {err}", window.cron)
            }
        }
    }
    model.pricing.default_prices.clone().or_else(|| model.pricing.prices.clone())
}

/// Aggregates the `llm.usage` audit entries of one run.
///
/// Models without pricing contribute token counts but zero cost.
pub fn run_usage(
    db: &crate::storage::persistence::Db,
    config: &BillingConfig,
    run_id: &str,
) -> DaemonResult<UsageSummaryData> {
    let writer = crate::observability::audit::AuditWriter::new(db.clone());
    let mut by_model: HashMap<String, ModelUsageSummary> = HashMap::new();
    let mut total = 0u64;
    let mut requested: HashMap<String, u64> = HashMap::new();

    for entry in writer.list()? {
        if !matches!(entry.operation.as_str(), "llm.usage" | "llm.request") {
            continue;
        }
        if entry.detail.get("run_id").and_then(|v| v.as_str()) != Some(run_id) {
            continue;
        }
        let get = |key: &str| entry.detail.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let model =
            entry.detail.get("model").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let slot = by_model.entry(model.clone()).or_insert_with(|| ModelUsageSummary {
            model: model.clone(),
            tokens_complete: true,
            cache_complete: true,
            cost_complete: true,
            calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            cached_input_tokens: 0,
            cache_write_input_tokens: 0,
            cost_micros: 0,
        });
        if entry.operation == "llm.request" {
            *requested.entry(model).or_default() += 1;
            continue;
        }
        let tokens_complete = entry.detail.get("tokens_reported").and_then(|v| v.as_bool()) == Some(true)
            && ["input_tokens", "output_tokens"].iter().all(|key| entry.detail.get(*key).and_then(|v| v.as_u64()).is_some());
        slot.tokens_complete &= tokens_complete;
        slot.cache_complete &= tokens_complete
            && entry.detail.get("cache_read_reported").and_then(|v| v.as_bool()) == Some(true)
            && entry.detail.get("cached_input_tokens").and_then(|v| v.as_u64()).is_some()
            && get("cached_input_tokens") <= get("input_tokens");
        slot.cost_complete &= tokens_complete && entry.detail.get("cost_micros").and_then(|v| v.as_u64()).is_some()
            && (get("reasoning_tokens") == 0 || get("accounting_version") == 1)
            && entry.detail.get("currency").and_then(|v| v.as_str()) == Some(if config.currency.is_empty() { "USD" } else { &config.currency });
        slot.calls += 1;
        slot.input_tokens += get("input_tokens");
        slot.output_tokens += get("output_tokens");
        slot.reasoning_tokens += get("reasoning_tokens");
        slot.cached_input_tokens += get("cached_input_tokens");
        slot.cache_write_input_tokens += get("cache_write_input_tokens");
        let cost_micros = get("cost_micros");
        slot.cost_micros += cost_micros;
        total += cost_micros;
    }

    for (model, count) in requested {
        if let Some(slot) = by_model.get_mut(&model) && count > slot.calls {
            slot.tokens_complete = false; slot.cache_complete = false; slot.cost_complete = false;
        }
    }
    // Independent reservations remain authoritative after a caller crash.
    if let Ok(run) = uuid::Uuid::parse_str(run_id) {
        for call in crate::oversight::budget::load(db, run)?.calls {
            let slot = by_model.entry(call.model.clone()).or_insert_with(|| ModelUsageSummary {
                model: call.model.clone(), tokens_complete: true, cache_complete: true,
                cost_complete: true, calls: 0, input_tokens: 0, output_tokens: 0,
                reasoning_tokens: 0, cached_input_tokens: 0, cache_write_input_tokens: 0, cost_micros: 0,
            });
            let usage = call.usage.unwrap_or_default();
            slot.calls += 1;
            slot.tokens_complete &= usage.tokens_reported;
            slot.cache_complete &= usage.tokens_reported && usage.cache_read_reported && usage.cached_input_tokens <= usage.input_tokens;
            slot.cost_complete &= usage.tokens_reported && (usage.reasoning_tokens == 0 || call.accounting_version == 1)
                && call.cost_micros.is_some() && call.currency == if config.currency.is_empty() { "USD" } else { &config.currency };
            slot.input_tokens += usage.input_tokens;
            slot.output_tokens += usage.output_tokens;
            slot.reasoning_tokens += usage.reasoning_tokens;
            slot.cached_input_tokens += usage.cached_input_tokens;
            slot.cache_write_input_tokens += usage.cache_write_input_tokens;
            slot.cost_micros += call.cost_micros.unwrap_or(0);
            total += call.cost_micros.unwrap_or(0);
        }
    }
    let mut models: Vec<ModelUsageSummary> = by_model.into_values().collect();
    models.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.model.cmp(&b.model)));
    Ok(UsageSummaryData {
        currency: if config.currency.is_empty() {
            "USD".to_string()
        } else {
            config.currency.clone()
        },
        total_cost_micros: total,
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use metteur_shared::Usage;
    use metteur_shared::config::{ModelPricingStrategy, PeakWindowPrice, PricingTier};

    fn pricing(input: f64, output: f64) -> ModelPricing {
        ModelPricing {
            input_per_mtok: input,
            output_per_mtok: output,
            ..Default::default()
        }
    }

    fn peak_model() -> LlmModelConfig {
        LlmModelConfig {
            pricing: ModelPricingStrategy {
                kind: "peak".to_string(),
                default_prices: Some(pricing(1.0, 1.0)),
                windows: vec![PeakWindowPrice {
                    cron: "0 22 * * *".to_string(),
                    duration_min: 360,
                    prices: pricing(0.5, 0.5),
                }],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, min: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(year, month, day, hour, min, 0)
            .single()
            .expect("valid test time")
    }

    #[test]
    fn cache_traffic_uses_the_cache_rates() {
        let mut p = pricing(1.0, 10.0);
        p.cache_hit_per_mtok = 0.1;
        p.cache_write_per_mtok = 1.25;
        let usage = Usage {
            tokens_reported: true,
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_input_tokens: 500_000,
            cache_write_input_tokens: 200_000,
            ..Usage::default()
        };
        let cost = cost(Some(&p), "USD", &usage).unwrap();
        // 0.3M uncached @ $1 + 0.5M hit @ $0.1 + 0.2M write @ $1.25 = $0.6.
        assert_eq!(cost.micros, 600_000);
    }

    #[test]
    fn missing_cache_rates_fall_back_to_the_input_rate() {
        let p = pricing(2.0, 10.0);
        let usage = Usage {
            tokens_reported: true,
            input_tokens: 1_000_000,
            cached_input_tokens: 1_000_000,
            ..Usage::default()
        };
        let cost = cost(Some(&p), "USD", &usage).unwrap();
        // Without a cache-hit rate the tokens bill as regular input.
        assert_eq!(cost.micros, 2_000_000);
    }

    #[test]
    fn computes_peak_cost() {
        let p = pricing(1.0, 10.0);
        let usage = Usage {
            tokens_reported: true,
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            reasoning_tokens: 0,
            total_tokens: 1_100_000,
            ..Usage::default()
        };
        let cost = cost(Some(&p), "USD", &usage).unwrap();
        assert_eq!(cost.micros, 2_000_000); // $1 input + $1 output
        assert_eq!(cost.currency, "USD");
    }

    #[test]
    fn unpriced_models_have_no_cost() {
        assert!(cost(None, "USD", &Usage::default()).is_none());
    }

    #[test]
    fn peak_window_selects_window_prices() {
        let model = peak_model();
        // Window 22:00-04:00 UTC.
        let night = effective_pricing_at(&model, 100, "UTC", utc(2026, 9, 9, 23, 30)).unwrap();
        assert_eq!(night.input_per_mtok, 0.5);
        let day = effective_pricing_at(&model, 100, "UTC", utc(2026, 9, 9, 12, 0)).unwrap();
        assert_eq!(day.input_per_mtok, 1.0);
    }

    #[test]
    fn unknown_timezone_falls_back_to_utc() {
        let model = peak_model();
        let night = effective_pricing_at(&model, 100, "Not/AZone", utc(2026, 9, 9, 23, 30)).unwrap();
        assert_eq!(night.input_per_mtok, 0.5);
    }

    #[test]
    fn invalid_cron_windows_are_skipped() {
        let mut model = peak_model();
        model.pricing.windows.insert(
            0,
            PeakWindowPrice {
                cron: "not a cron".to_string(),
                duration_min: 60,
                prices: pricing(9.0, 9.0),
            },
        );
        let night = effective_pricing_at(&model, 100, "UTC", utc(2026, 9, 9, 23, 30)).unwrap();
        assert_eq!(night.input_per_mtok, 0.5);
    }

    #[test]
    fn peak_without_windows_uses_default_prices() {
        let mut model = peak_model();
        model.pricing.windows.clear();
        let night = effective_pricing_at(&model, 100, "UTC", utc(2026, 9, 9, 23, 30)).unwrap();
        assert_eq!(night.input_per_mtok, 1.0);
    }

    #[test]
    fn tiered_resolves_the_covering_tier() {
        let model = LlmModelConfig {
            pricing: ModelPricingStrategy {
                kind: "tiered".to_string(),
                tiers: vec![PricingTier {
                    name: "base".to_string(),
                    max_input_tokens: Some(200_000),
                    prices: pricing(2.0, 4.0),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = effective_pricing_at(&model, 100, "UTC", utc(2026, 9, 9, 12, 0)).unwrap();
        assert_eq!(resolved.input_per_mtok, 2.0);
    }
    #[test]
    fn run_usage_preserves_completeness_and_pending_requests() {
        let dir = std::env::temp_dir().join(format!("metteur-usage-{}", uuid::Uuid::new_v4()));
        let db = crate::storage::persistence::Db::open(&dir).unwrap();
        let writer = crate::observability::audit::AuditWriter::new(db.clone());
        let config = BillingConfig::default();
        for (model, input, cached) in [("small", 100, 100), ("large", 900, 0)] {
            writer.record("test", "llm.usage", serde_json::json!({
                "run_id": "known", "model": model, "input_tokens": input, "output_tokens": 50,
                "cached_input_tokens": cached, "cache_write_input_tokens": 30,
                "tokens_reported": true, "cache_read_reported": true, "cost_micros": 1, "currency": "USD"
            })).unwrap();
        }
        let known = run_usage(&db, &config, "known").unwrap();
        assert_eq!(known.models.len(), 2);
        assert!(known.models.iter().all(|m| m.tokens_complete && m.cache_complete && m.cost_complete));
        let total = metteur_shared::Usage {
            tokens_reported: true, cache_read_reported: true,
            input_tokens: known.models.iter().map(|m| m.input_tokens).sum(),
            cached_input_tokens: known.models.iter().map(|m| m.cached_input_tokens).sum(),
            ..Default::default()
        };
        assert_eq!(total.cache_hit_rate(), Some(0.1));
        assert_eq!(run_usage(&db, &config, "known").unwrap().total_cost_micros, 2);
        assert!(run_usage(&db, &config, "other").unwrap().models.is_empty());
        for (run, detail) in [
            ("legacy", serde_json::json!({"input_tokens": 100, "cached_input_tokens": 0, "output_tokens": 3})),
            ("failed", serde_json::json!({"tokens_reported": false})),
            ("partial", serde_json::json!({"tokens_reported": true, "input_tokens": 100})),
        ] {
            let mut detail = detail; detail["run_id"] = run.into(); detail["model"] = "m".into();
            writer.record("test", "llm.usage", detail).unwrap();
            let result = run_usage(&db, &config, run).unwrap();
            assert!(!result.models[0].tokens_complete);
            assert!(!result.models[0].cache_complete);
            assert!(!result.models[0].cost_complete);
        }
        writer.record("test", "llm.request", serde_json::json!({"run_id": "pending", "model": "m"})).unwrap();
        let pending = run_usage(&db, &config, "pending").unwrap();
        assert_eq!(pending.models[0].calls, 0);
        assert!(!pending.models[0].cache_complete);
        writer.record("test", "llm.usage", serde_json::json!({
            "run_id": "known", "model": "small", "tokens_reported": false
        })).unwrap();
        assert!(run_usage(&db, &config, "known").unwrap().models.iter().any(|m| !m.cache_complete));
    }

}
