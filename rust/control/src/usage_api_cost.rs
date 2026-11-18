//! Preserve scalar display prices while accepting precise collector quantities.

use super::SourceCost;
use devcoordinator2_api::cost_component::{CostComponent, decode_cost_components};
use serde::{Deserialize, Deserializer, de::Error};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSourceCost {
    basis: String,
    currency: String,
    status: String,
    processing_tier: String,
    estimated_usd_micros: Option<u64>,
    input_usd_micros: Option<CostComponent>,
    cached_input_usd_micros: Option<CostComponent>,
    cache_write_usd_micros: Option<CostComponent>,
    output_usd_micros: Option<CostComponent>,
    #[serde(default)]
    reasoning_usd_micros: Option<CostComponent>,
    input_tokens: i64,
    uncached_input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: u64,
    output_tokens: i64,
    reasoning_tokens: i64,
    provider_total_tokens: u64,
    priced_observations: u64,
    unknown_observations: u64,
    rate_card_refs: Vec<String>,
    #[serde(default)]
    unavailable_reasons: BTreeMap<String, u64>,
}

impl<'de> Deserialize<'de> for SourceCost {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = WireSourceCost::deserialize(deserializer)?;
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
        let _ = decode_cost_components(
            &value.status,
            [value.reasoning_usd_micros, None, None, None],
        )
        .map_err(D::Error::custom)?;
        Ok(Self {
            basis: value.basis,
            currency: value.currency,
            status: value.status,
            processing_tier: value.processing_tier,
            estimated_usd_micros: value.estimated_usd_micros,
            input_usd_micros: input,
            cached_input_usd_micros: cached,
            cache_write_usd_micros: write,
            output_usd_micros: output,
            input_tokens: value.input_tokens,
            uncached_input_tokens: value.uncached_input_tokens,
            cached_input_tokens: value.cached_input_tokens,
            cache_write_tokens: value.cache_write_tokens,
            output_tokens: value.output_tokens,
            reasoning_tokens: value.reasoning_tokens,
            provider_total_tokens: value.provider_total_tokens,
            priced_observations: value.priced_observations,
            unknown_observations: value.unknown_observations,
            rate_card_refs: value.rate_card_refs,
            unavailable_reasons: value.unavailable_reasons,
        })
    }
}
