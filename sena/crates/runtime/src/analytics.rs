//! Local usage-based telemetry and token budget auto-tuning.

use std::collections::VecDeque;

/// Recommendation produced by the token tuner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBudgetRecommendation {
    pub recommended_tokens: usize,
    pub p95_tokens: usize,
}

/// Rolling-window token-usage tracker and auto-tuner.
pub struct TokenTuner {
    recent: VecDeque<usize>,
    window: usize,
    tune_every: usize,
    samples_since_tune: usize,
    min_tokens: usize,
    max_tokens: usize,
}

impl TokenTuner {
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

    pub fn record(
        &mut self,
        token_count: usize,
        current_limit: usize,
    ) -> Option<TokenBudgetRecommendation> {
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

    fn recommendation(&self, current_limit: usize) -> Option<TokenBudgetRecommendation> {
        if self.recent.is_empty() {
            return None;
        }

        let p95_tokens = percentile_95(&self.recent);
        let recommended_tokens = ((p95_tokens as f64 * 1.20).ceil() as usize)
            .max(self.min_tokens)
            .min(self.max_tokens);

        let delta = (recommended_tokens as i64 - current_limit as i64).unsigned_abs() as f64;
        let threshold = current_limit as f64 * 0.10;
        if delta > threshold {
            Some(TokenBudgetRecommendation {
                recommended_tokens,
                p95_tokens,
            })
        } else {
            None
        }
    }
}

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
        assert_eq!(percentile_95(&d), 190);
    }

    #[test]
    fn tuner_no_recommendation_below_window() {
        let mut tuner = TokenTuner::new(256, 4096);
        for _ in 0..9 {
            assert!(tuner.record(200, 512).is_none());
        }
    }

    #[test]
    fn tuner_recommends_lower_budget_when_usage_is_low() {
        let mut tuner = TokenTuner::new(256, 4096);
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(100, 512);
        }
        let recommendation = result.expect("expected a lower recommendation");
        assert!(recommendation.recommended_tokens < 512);
        assert!(recommendation.recommended_tokens >= 256);
        assert_eq!(recommendation.p95_tokens, 100);
    }

    #[test]
    fn tuner_recommends_higher_budget_when_responses_near_limit() {
        let mut tuner = TokenTuner::new(256, 4096);
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(490, 512);
        }
        let recommendation = result.expect("expected a higher recommendation");
        assert!(recommendation.recommended_tokens > 512);
        assert!(recommendation.recommended_tokens <= 4096);
        assert_eq!(recommendation.p95_tokens, 490);
    }

    #[test]
    fn tuner_no_recommendation_when_budget_already_correct() {
        let mut tuner = TokenTuner::new(256, 4096);
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(430, 512);
        }
        assert!(result.is_none());
    }

    #[test]
    fn tuner_respects_min_tokens_floor() {
        let mut tuner = TokenTuner::new(300, 4096);
        let mut result = None;
        for _ in 0..10 {
            result = tuner.record(10, 512);
        }
        let recommendation = result.expect("expected a recommendation");
        assert!(recommendation.recommended_tokens >= 300);
    }
}
