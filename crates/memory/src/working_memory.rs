//! In-RAM working memory for a single inference cycle.
//!
//! WorkingMemory holds the current ContextSnapshot plus a rolling window of
//! recent InferenceExchanges, enforcing a token budget by evicting the oldest
//! exchanges when the budget is exceeded. It is retained across multiple
//! responses within the same cycle and only reset when a new cycle begins.
//! NEVER persisted, NEVER passed to ech0.

use bus::events::ctp::ContextSnapshot;

/// A single prompt/response exchange within one inference cycle.
#[derive(Debug, Clone)]
pub struct InferenceExchange {
    pub prompt: String,
    pub response: String,
}

impl InferenceExchange {
    /// Heuristic token estimate: (prompt.len() + response.len()) / 4.
    pub fn token_estimate(&self) -> usize {
        (self.prompt.len() + self.response.len()) / 4
    }
}

/// In-RAM working memory for a single inference cycle.
pub struct WorkingMemory {
    context: Option<ContextSnapshot>,
    exchanges: Vec<InferenceExchange>,
    max_exchanges: usize,
    token_budget: usize,
    total_tokens: usize,
}

impl WorkingMemory {
    pub fn new(max_exchanges: usize, token_budget: usize) -> Self {
        debug_assert!(max_exchanges > 0, "max_exchanges must be > 0");
        debug_assert!(token_budget > 0, "token_budget must be > 0");
        Self {
            context: None,
            exchanges: Vec::with_capacity(max_exchanges.min(64)),
            max_exchanges,
            token_budget,
            total_tokens: 0,
        }
    }

    pub fn add_context(&mut self, snapshot: ContextSnapshot) {
        self.context = Some(snapshot);
    }

    pub fn context(&self) -> Option<&ContextSnapshot> {
        self.context.as_ref()
    }

    /// Add a new exchange, evicting oldest entries if budget or cap exceeded.
    pub fn add_exchange(&mut self, exchange: InferenceExchange) {
        let new_tokens = exchange.token_estimate();

        while self.exchanges.len() >= self.max_exchanges && !self.exchanges.is_empty() {
            let removed = self.exchanges.remove(0);
            self.total_tokens = self.total_tokens.saturating_sub(removed.token_estimate());
        }

        while self.total_tokens + new_tokens > self.token_budget && self.exchanges.len() > 1 {
            let removed = self.exchanges.remove(0);
            self.total_tokens = self.total_tokens.saturating_sub(removed.token_estimate());
        }

        self.total_tokens += new_tokens;
        self.exchanges.push(exchange);
    }

    pub fn clear(&mut self) {
        self.reset_cycle();
    }

    pub fn reset_cycle(&mut self) {
        self.exchanges.clear();
        self.context = None;
        self.total_tokens = 0;
    }

    pub fn total_tokens(&self) -> usize {
        self.total_tokens
    }

    pub fn exchange_count(&self) -> usize {
        self.exchanges.len()
    }

    pub fn exchanges(&self) -> &[InferenceExchange] {
        &self.exchanges
    }

    /// Produce text chunks for prompt assembly (oldest-first).
    pub fn to_chunks(&self) -> Vec<String> {
        let mut chunks = Vec::new();

        if let Some(context) = &self.context {
            let mut lines = vec![format!(
                "Active application: {}",
                context.active_app.app_name
            )];

            if let Some(task) = &context.inferred_task {
                lines.push(format!(
                    "Inferred task: {} ({:.0}%)",
                    task.category,
                    task.confidence * 100.0
                ));
            }

            if context.clipboard_digest.is_some() {
                lines.push("Clipboard digest available".to_string());
            }

            lines.push(format!(
                "Keyboard cadence: {:.1} events/min",
                context.keystroke_cadence.events_per_minute
            ));

            chunks.push(format!("Context:\n{}", lines.join("\n")));
        }

        chunks.extend(
            self.exchanges
                .iter()
                .map(|e| format!("Prompt: {}\nResponse: {}", e.prompt, e.response)),
        );

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use std::time::Duration;
    use std::time::Instant;

    fn make_snap() -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "Test".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
            },
            session_duration: Duration::from_secs(0),
            inferred_task: None,
            timestamp: now,
        }
    }

    fn ex(p: &str, r: &str) -> InferenceExchange {
        InferenceExchange {
            prompt: p.to_string(),
            response: r.to_string(),
        }
    }

    #[test]
    fn working_memory_starts_empty() {
        let wm = WorkingMemory::new(10, 4096);
        assert_eq!(wm.exchange_count(), 0);
        assert_eq!(wm.total_tokens(), 0);
        assert!(wm.context().is_none());
    }

    #[test]
    fn add_context_stores_snapshot() {
        let mut wm = WorkingMemory::new(10, 4096);
        wm.add_context(make_snap());
        assert_eq!(wm.context().unwrap().active_app.app_name, "Test");
    }

    #[test]
    fn add_exchange_updates_count_and_tokens() {
        let mut wm = WorkingMemory::new(10, 4096);
        let e = ex("abcd", "efgh"); // (4+4)/4 = 2 tokens
        wm.add_exchange(e);
        assert_eq!(wm.exchange_count(), 1);
        assert_eq!(wm.total_tokens(), 2);
    }

    #[test]
    fn max_exchanges_evicts_oldest() {
        let mut wm = WorkingMemory::new(3, 100_000);
        for i in 0..5u64 {
            wm.add_exchange(ex(&format!("p{}", i), &format!("r{}", i)));
        }
        assert_eq!(wm.exchange_count(), 3);
        assert_eq!(wm.exchanges()[0].prompt, "p2");
    }

    #[test]
    fn token_budget_evicts_oldest() {
        // Each exchange: "aaaa"(4) + "bbbb"(4) = 8 chars -> 2 tokens
        let mut wm = WorkingMemory::new(100, 5);
        for _ in 0..5 {
            wm.add_exchange(ex("aaaa", "bbbb"));
        }
        assert!(wm.total_tokens() <= 7);
        assert!(wm.exchange_count() <= 4);
    }

    #[test]
    fn clear_resets_all_state() {
        let mut wm = WorkingMemory::new(10, 4096);
        wm.add_context(make_snap());
        wm.add_exchange(ex("a", "b"));
        wm.clear();
        assert_eq!(wm.exchange_count(), 0);
        assert_eq!(wm.total_tokens(), 0);
        assert!(wm.context().is_none());
    }

    #[test]
    fn to_chunks_returns_formatted_pairs() {
        let mut wm = WorkingMemory::new(10, 4096);
        wm.add_context(make_snap());
        wm.add_exchange(ex("hello", "world"));
        let chunks = wm.to_chunks();
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("Active application: Test"));
        assert!(chunks[1].contains("hello") && chunks[1].contains("world"));
    }

    #[test]
    fn working_memory_retains_multiple_responses_until_cycle_reset() {
        let mut wm = WorkingMemory::new(10, 4096);

        wm.add_context(make_snap());
        wm.add_exchange(ex("first prompt", "first response"));
        wm.add_exchange(ex("second prompt", "second response"));

        assert_eq!(wm.exchange_count(), 2);
        assert!(wm.context().is_some());

        let chunks = wm.to_chunks();
        assert_eq!(chunks.len(), 3);
        assert!(chunks[1].contains("first prompt"));
        assert!(chunks[2].contains("second prompt"));

        wm.reset_cycle();
        assert!(wm.context().is_none());
        assert_eq!(wm.exchange_count(), 0);
    }

    #[test]
    fn inference_exchange_token_estimate() {
        let e = InferenceExchange {
            prompt: "abcd".to_string(),
            response: "efgh".to_string(),
        };
        assert_eq!(e.token_estimate(), 2);
    }

    fn assert_send<T: Send>() {}
    #[test]
    fn types_are_send() {
        assert_send::<WorkingMemory>();
        assert_send::<InferenceExchange>();
    }
}
