//! Strict collector input adapters; public cost output remains scalar or null.

use crate::results::UsageCost;
use serde::{Deserialize, Deserializer, de::Error};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct CostComponent(WireComponent);

#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum WireComponent {
    Scalar(u64),
    Measured(MeasuredComponent),
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeasuredComponent {
    measured: u64,
    // Unlike a legacy optional component, a measurement must state precision.
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    exact: Option<u64>,
    unknown: u64,
}

/// Decode the four disjoint prices without inventing an aggregate or zero.
/// Inexact subtotals require a visibly incomplete cost status.
pub fn decode_cost_components(
    status: &str,
    components: [Option<CostComponent>; 4],
) -> Result<[Option<u64>; 4], &'static str> {
    let mut amounts = [None; 4];
    for (index, component) in components.into_iter().enumerate() {
        amounts[index] = match component.map(|component| component.0) {
            None => None,
            Some(WireComponent::Scalar(value)) => Some(value),
            Some(WireComponent::Measured(value)) => {
                if value
                    .exact
                    .is_some_and(|exact| exact != value.measured || value.unknown != 0)
                {
                    return Err("inconsistent cost component precision");
                }
                if value.exact.is_none() {
                    if !matches!(status, "partial" | "unavailable") {
                        return Err("inexact cost component requires incomplete pricing");
                    }
                    (value.measured != 0).then_some(value.measured)
                } else {
                    Some(value.measured)
                }
            }
        };
    }
    amounts
        .iter()
        .flatten()
        .try_fold(0u64, |sum, amount| sum.checked_add(*amount))
        .ok_or("cost component sum exceeds the supported integer range")?;
    Ok(amounts)
}

// Keep this wire shape aligned with UsageCost. Its four component inputs may
// carry precision; the API struct and generated output schema stay unchanged.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireUsageCost {
    status: String,
    basis: String,
    currency: String,
    #[serde(default)]
    processing_tier: String,
    estimated_usd: Option<String>,
    estimated_usd_micros: Option<u64>,
    input_usd_micros: Option<CostComponent>,
    cached_input_usd_micros: Option<CostComponent>,
    cache_write_usd_micros: Option<CostComponent>,
    output_usd_micros: Option<CostComponent>,
    // The collector reports reasoning cost as a fifth measured component.
    // The public Console contract does not expose this field, but the
    // consumer must still validate and accept its wire shape.
    #[serde(default)]
    reasoning_usd_micros: Option<CostComponent>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_tokens: Option<u64>,
    uncached_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    unknown_requests: Option<u64>,
    #[serde(default)]
    unknown_tokens: u64,
    unknown_observations: Option<u64>,
    priced_observations: Option<u64>,
    rate_card_ref: Option<String>,
    #[serde(default)]
    rate_card_refs: Vec<String>,
    #[serde(default)]
    matched_rate_cards: Vec<crate::rate_card::RateCard>,
    model_requests: Option<u64>,
    priced_requests: Option<u64>,
    #[serde(default)]
    unavailable_reasons: BTreeMap<String, u64>,
}

impl<'de> Deserialize<'de> for UsageCost {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = WireUsageCost::deserialize(deserializer)?;
        let [input, cached, write, output] = decode_cost_components(
            &value.status,
            [
                value.input_usd_micros,
                value.cached_input_usd_micros,
                value.cache_write_usd_micros,
                value.output_usd_micros,
            ],
        )
        .map_err(D::Error::custom)?;
        // Validate the producer's reasoning component even though the public
        // response intentionally keeps the historical four-price shape.
        let _ = decode_cost_components(
            &value.status,
            [value.reasoning_usd_micros, None, None, None],
        )
        .map_err(D::Error::custom)?;
        Ok(Self {
            status: value.status,
            basis: value.basis,
            currency: value.currency,
            processing_tier: value.processing_tier,
            estimated_usd: value.estimated_usd,
            estimated_usd_micros: value.estimated_usd_micros,
            input_usd_micros: input,
            cached_input_usd_micros: cached,
            cache_write_usd_micros: write,
            output_usd_micros: output,
            input_tokens: value.input_tokens,
            cached_input_tokens: value.cached_input_tokens,
            cache_write_tokens: value.cache_write_tokens,
            uncached_input_tokens: value.uncached_input_tokens,
            output_tokens: value.output_tokens,
            reasoning_tokens: value.reasoning_tokens,
            unknown_requests: value.unknown_requests,
            unknown_tokens: value.unknown_tokens,
            unknown_observations: value.unknown_observations,
            priced_observations: value.priced_observations,
            rate_card_ref: value.rate_card_ref,
            rate_card_refs: value.rate_card_refs,
            matched_rate_cards: value.matched_rate_cards,
            model_requests: value.model_requests,
            priced_requests: value.priced_requests,
            unavailable_reasons: value.unavailable_reasons,
        })
    }
}
