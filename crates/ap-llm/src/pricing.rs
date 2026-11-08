//! Per-model prices (USD per million tokens) for a cost estimate.
//!
//! Source: claude-api skill model table (cached 2026-10-06). Cache writes
//! use the 5-minute TTL rate, 1.25x the input price. The Haiku 5.5 cache-read
//! price is not in the table and is assumed at 0.1x input. Haiku 5.5 prices
//! are for prompts up to 100K tokens. Treat results as estimates.

use crate::types::Usage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

impl Price {
    const fn claude(input: f64, output: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_write: input * 1.25,
            cache_read,
        }
    }

    /// Estimated cost of `u` in USD.
    pub fn cost(&self, u: &Usage) -> f64 {
        (u.input_tokens as f64 * self.input
            + u.output_tokens as f64 * self.output
            + u.cache_creation_input_tokens as f64 * self.cache_write
            + u.cache_read_input_tokens as f64 * self.cache_read)
            / 1_000_000.0
    }
}

/// Price for a model id, if known. Unknown models (OpenAI-compatible, local) give `None`.
pub fn price(model: &str) -> Option<Price> {
    Some(match model {
        "claude-opus-5-5" => Price::claude(4.0, 20.0, 0.20),
        "claude-sonnet-5-5" => Price::claude(2.0, 10.0, 0.20),
        "claude-haiku-5-5" => Price::claude(0.10, 0.50, 0.01),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
            Price::claude(5.0, 25.0, 0.50)
        }
        "claude-sonnet-5" => Price::claude(2.0, 10.0, 0.20),
        "claude-sonnet-4-6" => Price::claude(3.0, 15.0, 0.30),
        "claude-haiku-4-5" => Price::claude(1.0, 5.0, 0.10),
        _ => return None,
    })
}

/// Estimated cost in USD; `None` for unknown models.
pub fn estimate(model: &str, u: &Usage) -> Option<f64> {
    price(model).map(|p| p.cost(u))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_cost() {
        let u = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 1_000_000,
        };
        let c = estimate("claude-opus-5-5", &u).unwrap();
        assert!((c - (4.0 + 2.0 + 0.2)).abs() < 1e-9);
        assert!(estimate("llava", &u).is_none());
    }
}
