use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Optional runtime measurements from a completed generation.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GenerationTiming {
    pub cached_prompt_tokens: i64,
    pub processed_prompt_tokens: i64,
    pub generated_tokens: i64,
    pub prompt_micros: i64,
    pub generation_micros: i64,
}

impl GenerationTiming {
    pub(super) fn from_llama(timing: &Value) -> Option<Self> {
        Some(Self {
            cached_prompt_tokens: count(&timing["cache_n"])?,
            processed_prompt_tokens: count(&timing["prompt_n"])?,
            generated_tokens: count(&timing["predicted_n"])?,
            prompt_micros: micros(&timing["prompt_ms"])?,
            generation_micros: micros(&timing["predicted_ms"])?,
        })
    }
}

fn count(value: &Value) -> Option<i64> {
    value.as_i64().filter(|count| *count >= 0)
}

fn micros(value: &Value) -> Option<i64> {
    let duration = Duration::try_from_secs_f64(value.as_f64()? / 1_000.0).ok()?;
    duration.as_micros().try_into().ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn timing_is_optional_and_requires_nonnegative_numbers_in_range() {
        let valid = json!({
            "cache_n": 0, "prompt_n": 11, "predicted_n": 1,
            "prompt_ms": 12.5, "predicted_ms": 0
        });
        assert_eq!(
            GenerationTiming::from_llama(&valid),
            Some(GenerationTiming {
                cached_prompt_tokens: 0,
                processed_prompt_tokens: 11,
                generated_tokens: 1,
                prompt_micros: 12_500,
                generation_micros: 0,
            })
        );
        assert_eq!(GenerationTiming::from_llama(&Value::Null), None);
        for (key, invalid) in [
            ("cache_n", json!(-1)),
            ("prompt_n", json!("11")),
            ("predicted_n", json!(u64::MAX)),
            ("prompt_ms", json!(-1)),
            ("predicted_ms", json!(1e16)),
        ] {
            let mut timing = valid.clone();
            timing[key] = invalid;
            assert_eq!(GenerationTiming::from_llama(&timing), None, "{key}");
        }
    }
}
