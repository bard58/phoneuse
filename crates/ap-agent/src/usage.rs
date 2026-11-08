//! Token usage and cost estimate per LLM call.

use ap_llm::Usage;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepUsage {
    pub step: u32,
    pub model: String,
    pub usage: Usage,
    /// `None` when the model has no known price.
    pub cost_usd: Option<f64>,
}

/// Running log of usage for one agent run.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageLog {
    pub steps: Vec<StepUsage>,
    pub total: Usage,
    /// Sum of known step costs; `None` if no step had a known price.
    pub total_cost_usd: Option<f64>,
}

impl UsageLog {
    pub fn record(&mut self, step: u32, model: &str, usage: Usage) -> &StepUsage {
        let cost = ap_llm::pricing::estimate(model, &usage);
        self.total.add(&usage);
        if let Some(c) = cost {
            *self.total_cost_usd.get_or_insert(0.0) += c;
        }
        self.steps.push(StepUsage {
            step,
            model: model.to_string(),
            usage,
            cost_usd: cost,
        });
        self.steps.last().expect("just pushed")
    }

    /// One line for logs: tokens, cache share, cost.
    pub fn summary(&self) -> String {
        let t = &self.total;
        let cost = self
            .total_cost_usd
            .map(|c| format!(", ~${c:.4}"))
            .unwrap_or_default();
        format!(
            "{} calls, input {} (cache read {}, cache write {}), output {}{cost}",
            self.steps.len(),
            t.total_input(),
            t.cache_read_input_tokens,
            t.cache_creation_input_tokens,
            t.output_tokens
        )
    }
}
