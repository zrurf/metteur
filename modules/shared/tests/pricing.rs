use metteur_shared::config::{ConfigLayer, ModelPricing, ModelPricingStrategy};
use serde_json::json;

#[test]
fn pricing_preflight_rejects_invalid_thresholds_and_rates() {
    for tiers in [
        json!([]),
        json!([{"max_input_tokens":0}]),
        json!([{"max_input_tokens":100},{"max_input_tokens":100}]),
        json!([{"max_input_tokens":101},{"max_input_tokens":100}]),
        json!([{}, {"max_input_tokens":100}]),
    ] {
        let layer: ConfigLayer = serde_json::from_value(
            json!({"llm":{"models":{"m":{"pricing":{"kind":"tiered","tiers":tiers}}}}}),
        )
        .unwrap();
        assert!(layer.effective().is_err());
    }
    for key in ["input_per_mtok", "output_per_mtok", "cache_hit_per_mtok", "cache_write_per_mtok"] {
        let layer: ConfigLayer = serde_json::from_value(
            json!({"llm":{"models":{"m":{"pricing":{"prices":{key:-0.1}}}}}}),
        )
        .unwrap();
        assert!(layer.effective().is_err());
    }
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert!(
            ModelPricing {
                input_per_mtok: invalid,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    let valid: ModelPricingStrategy = serde_json::from_value(
        json!({"kind":"tiered","tiers":[{"max_input_tokens":100},{"max_input_tokens":200},{}]}),
    )
    .unwrap();
    assert!(valid.validate().is_ok());
}
