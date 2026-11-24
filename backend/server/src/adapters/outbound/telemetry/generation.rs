use ctxfab_inference::generation::GenerationTiming;
use opentelemetry::{KeyValue, Value};

use crate::application::telemetry::GenerationContext;

const CACHED: &str = "inference.prompt.cached_tokens";
const PROCESSED: &str = "inference.prompt.processed_tokens";
const GENERATED: &str = "inference.generated_tokens";
const PROMPT: &str = "inference.prompt.duration_us";
const DECODE: &str = "inference.decode.duration_us";
const BUDGET: &str = "ctxfab.inference.context_budget";
const MAX_BUDGET_BYTES: usize = 512;

pub(super) fn context_attribute(context: &GenerationContext) -> Option<KeyValue> {
    let encoded = serde_json::to_string(context).ok()?;
    (encoded.len() <= MAX_BUDGET_BYTES).then(|| KeyValue::new(BUDGET, encoded))
}

pub(super) fn read_context(attributes: &[KeyValue]) -> Option<GenerationContext> {
    let entry = attributes
        .iter()
        .find(|entry| entry.key.as_str() == BUDGET)?;
    let Value::String(encoded) = &entry.value else {
        return None;
    };
    let encoded = encoded.as_str();
    if encoded.len() > MAX_BUDGET_BYTES {
        return None;
    }
    serde_json::from_str(encoded).ok()
}

pub(super) fn attributes(timing: GenerationTiming) -> [KeyValue; 5] {
    [
        KeyValue::new(CACHED, timing.cached_prompt_tokens),
        KeyValue::new(PROCESSED, timing.processed_prompt_tokens),
        KeyValue::new(GENERATED, timing.generated_tokens),
        KeyValue::new(PROMPT, timing.prompt_micros),
        KeyValue::new(DECODE, timing.generation_micros),
    ]
}

pub(super) fn read(attributes: &[KeyValue]) -> Option<GenerationTiming> {
    let count = |key| {
        let attribute = attributes.iter().find(|entry| entry.key.as_str() == key)?;
        match attribute.value {
            Value::I64(value) if value >= 0 => Some(value),
            _ => None,
        }
    };
    Some(GenerationTiming {
        cached_prompt_tokens: count(CACHED)?,
        processed_prompt_tokens: count(PROCESSED)?,
        generated_tokens: count(GENERATED)?,
        prompt_micros: count(PROMPT)?,
        generation_micros: count(DECODE)?,
    })
}
