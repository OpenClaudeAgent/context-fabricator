use std::num::NonZeroUsize;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::GenerationError;
use crate::supervisor::{Client, WORKER_CONTEXT_TOKENS};

pub(super) const OUTPUT_TOKENS: usize = 640;

#[derive(Deserialize)]
struct Count {
    input_tokens: NonZeroUsize,
}

/// Measurements for the prefix selected for completion, or for an exhausted budget.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBudget {
    pub proposed: usize,
    pub admitted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<NonZeroUsize>,
    pub input_token_limit: usize,
    pub measurements: usize,
}

pub(super) enum Plan {
    ByCount(RangeInclusive<usize>),
    RankPrefixFirst(usize),
}

impl From<RangeInclusive<usize>> for Plan {
    fn from(counts: RangeInclusive<usize>) -> Self {
        Self::ByCount(counts)
    }
}

impl Plan {
    fn proposed(&self) -> usize {
        match self {
            Self::ByCount(counts) => *counts.end(),
            Self::RankPrefixFirst(count) => *count,
        }
    }

    fn layouts(self) -> Vec<(usize, bool)> {
        match self {
            Self::ByCount(counts) => counts
                .rev()
                .flat_map(|count| [(count, true), (count, false)])
                .collect(),
            Self::RankPrefixFirst(count) => (2..=count)
                .rev()
                .map(|count| (count, true))
                .chain((2..count).rev().map(|count| (count, false)))
                .chain((count > 0).then_some((1, false)))
                .collect(),
        }
    }
}

pub(super) fn fit(
    client: &Client,
    model: &str,
    cancelled: &AtomicBool,
    plan: Plan,
    render: impl Fn(usize, bool) -> Option<Value>,
) -> Result<(ContextBudget, Option<(Value, bool)>), GenerationError> {
    let context = client.context_tokens(model)?;
    let limit = context
        .min(WORKER_CONTEXT_TOKENS)
        .saturating_sub(OUTPUT_TOKENS + 1);
    let mut budget = ContextBudget {
        proposed: plan.proposed(),
        admitted: 0,
        input_tokens: None,
        input_token_limit: limit,
        measurements: 0,
    };
    if limit == 0 {
        return Ok((budget, None));
    }
    for (count, preferred) in plan.layouts() {
        let Some(payload) = render(count, preferred) else {
            continue;
        };
        let bytes = client.post("/v1/chat/completions/input_tokens", &payload)?;
        let measured: Count =
            serde_json::from_slice(&bytes).map_err(|_| GenerationError::Output)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(GenerationError::Cancelled);
        }
        budget.measurements += 1;
        if measured.input_tokens.get() <= limit {
            budget.admitted = count;
            budget.input_tokens = Some(measured.input_tokens);
            return Ok((budget, Some((payload, preferred))));
        }
    }
    Ok((budget, None))
}
