use super::{ModelPricing, ModelPricingStrategy};

impl ModelPricing {
    /// Rejects invalid rates before configuration can become effective.
    pub fn validate(&self) -> Result<(), &'static str> {
        if [
            self.input_per_mtok,
            self.output_per_mtok,
            self.cache_hit_per_mtok,
            self.cache_write_per_mtok,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
        {
            Ok(())
        } else {
            Err("model prices must be finite and non-negative")
        }
    }
}

impl ModelPricingStrategy {
    /// Tier bounds are inclusive, strictly increasing, with an optional final unbounded tier.
    pub fn validate(&self) -> Result<(), &'static str> {
        for prices in self
            .prices
            .iter()
            .chain(self.default_prices.iter())
            .chain(self.tiers.iter().map(|tier| &tier.prices))
            .chain(self.windows.iter().map(|window| &window.prices))
        {
            prices.validate()?;
        }
        if self.kind == "tiered" && self.tiers.is_empty() {
            return Err("tiered pricing requires at least one tier");
        }
        let mut previous = None;
        for (index, tier) in self.tiers.iter().enumerate() {
            match tier.max_input_tokens {
                Some(bound) if bound > 0 && previous.is_none_or(|prior| bound > prior) => {
                    previous = Some(bound)
                }
                None if index + 1 == self.tiers.len() => {}
                _ => {
                    return Err("pricing thresholds must increase; an unbounded tier must be last");
                }
            }
        }
        Ok(())
    }
}
