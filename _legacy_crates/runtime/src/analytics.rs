//! Local usage-based telemetry and token budget auto-tuning.
//!
//! Tracks `token_count` from every `InferenceCompleted` event in a rolling
//! window and computes a P95 recommendation with 20% headroom. When the
//! computed recommendation differs from the current `inference_max_tokens` by
//! more than 10%, it writes the new value to the config file and emits
//! `SystemEvent::TokenBudgetAutoTuned` on the bus.
//!
//! This module has no external dependencies and performs no network calls.
//! All data stays on-device and in-process.

use std::collections::VecDeque;

/// Rolling-window token-usage tracker and auto-tuner.
///
/// - Observation window: last `window` inferences.
/// - Re-evaluation every `tune_every` new samples.
/// - Budget = P95(window) * 1.20, clamped to [min, max].
/// - A new budget is proposed only when it differs from the current by >10%.
pub struct TokenTuner {
    recent: VecDeque<usize>,
    window: usize,
    tune_every: usize,
    samples_since_tune: usize,
    min_tokens: usize,
    max_tokens: usize,
}

impl TokenTuner {
    /// Create a tuner with the given bounds from config.
    pub fn new(min_tokens: usize, max_tokens: usize) -> Self {
        Self {
            recent: VecDeque::new(),
            window: 50,
            tune_every: 10,
            samples_since_tune: 0,
            min_tokens,
            max_tokens,
        }
    }

    /// Record an observed token count from a completed inference.
    ///
    /// Returns `Some(recommended)` if enough data has accumulated and the
    /// recommended value differs from `current_limit` by more than 10%.
    /// Returns `None` otherwise.
    pub fn record(&mut self, token_count: usize, current_limit: usize) -> Option<usize> {
        if self.recent.len() >= self.window {
            self.recent.pop_front();
        }
        self.recent.push_back(token_count);
        self.samples_since_tune += 1;

        if self.samples_since_tune >= self.tune_every && self.recent.len() >= self.tune_every {
            self.samples_since_tune = 0;
            self.recommendation(current_limit)
        } else {
            None
        }
    }

    /// Compute a recommendation given the current token limit.
    ///
    /// Returns `None` when the window is too small or the delta is within 10%.
    fn recommendation(&self, current_limit: usize) -> Option<usize> {
        if self.recent.is_empty() {
            return None;
        }

        let p95 = percentile_95(&self.recent);
        // Add 20% headroom so responses have room without truncation.
        let recommended = ((p95 as f64 * 1.20).ceil() as usize)
            .max(self.min_tokens)
            .min(self.max_tokens);

        // Only act if the delta is >10% to avoid thrashing.
        let delta = (recommended as i64 - current_limit as i64).unsigned_abs() as f64;
        let threshold = current_limit as f64 * 0.10;
        if delta > threshold {
            Some(recommended)
        } else {
            None
        }
    }

    /// Number of observations in the current window.
    pub fn window_size(&self) -> usize {
        self.recent.len()
    }
}

/// Compute the 95th percentile of a token count window.
fn percentile_95(counts: &VecDeque<usize>) -> usize {
    if counts.is_empty() {
        return 0;
    }
    let mut sorted: Vec<usize> = counts.iter().copied().collect();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_95_single_value() {
        let mut d = VecDeque::new();
        d.push_back(100usize);
        assert_eq!(percentile_95(&d), 100);
    }

    #[test]
    fn percentile_95_returns_high_value() {
        let mut d = VecDeque::new();
        for i in 1..=20 {
            d.push_back(i * 10);
        }
        // Sorted: 10..200. P95 index = ceil(20*0.95)-1 = 19-1=18 → value 190.
        assert_eq!(percentile_95(&d), 190);
    }

    #[test]
    fn tuner_no_recommendation_below_window() {
        let mut tuner = TokenTuner::new(256, 4096);
        // Feed 9 samples (tune_every=10) — should produce no recommendation.
        for _ in 0..9 {
            assert!(tuner.record(200, 512).is_none());
        }
    }

    #[test]
    fn tuner_recommends_lower_budget_when_usage_is_low() {
        let mut tuner = TokenTuner::new(256, 4096);
        // Feed 10 samples of 100 tokens each.
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(100, 512);
        }
        // P95 = 100, recommended = ceil(100*1.2) = 120.
        // 120 vs 512 → delta = 392 > 51.2 (10%) → should recommend.
        let r = result.unwrap();
        assert!(r < 512, "expected lower recommendation, got {}", r);
        assert!(r >= 256, "expected to respect min_tokens, got {}", r);
    }

    #[test]
    fn tuner_recommends_higher_budget_when_responses_near_limit() {
        let mut tuner = TokenTuner::new(256, 4096);
        // Feed 10 samples very close to the current limit of 512.
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(490, 512);
        }
        // P95 = 490, recommended = ceil(490*1.2) = 588.
        // 588 vs 512 → delta = 76 > 51.2 (10%) → should recommend.
        let r = result.unwrap();
        assert!(r > 512, "expected higher recommendation, got {}", r);
        assert!(r <= 4096, "expected to respect max_tokens, got {}", r);
    }

    #[test]
    fn tuner_no_recommendation_when_budget_already_correct() {
        let mut tuner = TokenTuner::new(256, 4096);
        // P95 ≈ 430, recommended ≈ 516 ≈ current 512 (within 10%).
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(430, 512);
        }
        // P95 = 430, recommended = ceil(430*1.2) = 516.
        // delta = |516-512| = 4 < 51.2 → no recommendation.
        assert!(
            result.is_none(),
            "expected no recommendation, got {:?}",
            result
        );
    }

    #[test]
    fn tuner_respects_min_tokens_floor() {
        let mut tuner = TokenTuner::new(300, 4096);
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(10, 512); // tiny responses
        }
        let r = result.unwrap();
        assert!(r >= 300, "expected min_tokens floor respected, got {}", r);
    }
}
