//! LLM cost model for an agent loop.
//!
//! In a tool-using loop every turn re-reads the whole conversation, so context cost grows
//! roughly quadratically with the number of calls and with the size of each tool result.
//! That is why fewer calls and smaller outputs matter more than they first appear.

use clap::Args;

#[derive(Args, Clone)]
pub struct CostArgs {
    /// USD per million input tokens. ILLUSTRATIVE default; set your model's real price.
    #[arg(long, default_value_t = 3.0)]
    pub price_in: f64,
    /// USD per million output tokens. ILLUSTRATIVE default.
    #[arg(long, default_value_t = 15.0)]
    pub price_out: f64,
    /// Seconds an LLM turn takes (time to first action). ILLUSTRATIVE default.
    #[arg(long, default_value_t = 3.0)]
    pub llm_turn_secs: f64,
    /// Tokens already in context before the first call (system prompt + task).
    #[arg(long, default_value_t = 1500)]
    pub base_context: u64,
}

#[derive(Clone)]
pub struct Call {
    pub args_tokens: u64,
    pub out_tokens: u64,
}

pub struct Estimate {
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd_uncached: f64,
    pub usd_cached: f64,
}

/// `schema_tokens` is the tool-definition overhead carried in every turn.
pub fn estimate(calls: &[Call], schema_tokens: u64, a: &CostArgs) -> Estimate {
    const ASSISTANT_OVERHEAD: u64 = 30; // tokens the model spends around each tool call
    let turns = calls.len() as u64 + 1;
    let mut ctx = a.base_context + schema_tokens;
    let mut prev_ctx = 0u64;
    let (mut input, mut output) = (0u64, 0u64);
    let (mut cached_cost_units, mut uncached_units) = (0f64, 0f64);
    for i in 0..turns as usize {
        input += ctx;
        uncached_units += ctx as f64;
        // prompt caching: previously-seen tokens are read at 0.1x, new tokens written at 1.25x
        cached_cost_units += prev_ctx as f64 * 0.10 + (ctx - prev_ctx) as f64 * 1.25;
        prev_ctx = ctx;
        if let Some(c) = calls.get(i) {
            let gen = c.args_tokens + ASSISTANT_OVERHEAD;
            output += gen;
            ctx += gen + c.out_tokens;
        } else {
            output += 80; // final answer
        }
    }
    let out_usd = output as f64 * a.price_out / 1e6;
    Estimate {
        turns,
        input_tokens: input,
        output_tokens: output,
        usd_uncached: uncached_units * a.price_in / 1e6 + out_usd,
        usd_cached: cached_cost_units * a.price_in / 1e6 + out_usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> CostArgs {
        CostArgs {
            price_in: 3.0,
            price_out: 15.0,
            llm_turn_secs: 3.0,
            base_context: 1000,
        }
    }

    #[test]
    fn more_calls_cost_superlinearly() {
        let c = |n: usize| {
            (0..n)
                .map(|_| Call {
                    args_tokens: 20,
                    out_tokens: 300,
                })
                .collect::<Vec<_>>()
        };
        let e5 = estimate(&c(5), 1500, &args());
        let e10 = estimate(&c(10), 1500, &args());
        assert!(
            e10.input_tokens > e5.input_tokens * 2,
            "{} vs {}",
            e10.input_tokens,
            e5.input_tokens
        );
    }

    #[test]
    fn caching_never_costs_more_than_a_few_percent_over_uncached_for_long_loops() {
        let calls: Vec<Call> = (0..12)
            .map(|_| Call {
                args_tokens: 20,
                out_tokens: 400,
            })
            .collect();
        let e = estimate(&calls, 1500, &args());
        assert!(e.usd_cached < e.usd_uncached);
    }

    #[test]
    fn zero_calls_is_one_turn() {
        let e = estimate(&[], 0, &args());
        assert_eq!(e.turns, 1);
        assert_eq!(e.input_tokens, 1000);
    }
}
