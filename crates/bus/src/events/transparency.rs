//! Transparency query events — PR Principle P7: "Earn trust through transparency"
//!
//! Users can ask Sena three fundamental questions:
//! 1. "What are you observing right now?" — Current context signals
//! 2. "What do you remember about me?" — Accumulated personalization state
//! 3. "Why did you say that?" — Reasoning chain for last inference
//!
//! These events define the protocol between CLI and the three responder actors.

use crate::events::ctp::ContextSnapshot;
use crate::events::memory::MemoryChunk;

/// User query for transparency: ask Sena about its current state.
#[derive(Debug, Clone)]
pub enum TransparencyQuery {
    /// "What are you observing right now?"
    CurrentObservation,
    /// "What do you remember about me?"
    UserMemory,
    /// "Why did you say that?"
    InferenceExplanation,
}

/// Response to CurrentObservation query: current context snapshot.
#[derive(Debug, Clone)]
pub struct ObservationResponse {
    pub snapshot: ContextSnapshot,
}

/// Response to UserMemory query: soul summary + recent memory chunks.
#[derive(Debug, Clone)]
pub struct MemoryResponse {
    /// High-level summary of Sena's understanding of the user.
    pub soul_summary: SoulSummaryForTransparency,
    /// Recent and important memory nodes, ranked by relevance.
    pub memory_chunks: Vec<MemoryChunk>,
}

/// A redacted view of SoulBox state for transparency purposes.
/// Only includes high-level aggregates, never raw identity data.
#[derive(Debug, Clone)]
pub struct SoulSummaryForTransparency {
    /// Number of inference cycles completed.
    pub inference_cycle_count: usize,
    /// Top work patterns observed (e.g., "morning_coder", "late_night_writer").
    pub work_patterns: Vec<String>,
    /// Top tool preferences (e.g., "vscode", "cargo", "chrome").
    pub tool_preferences: Vec<String>,
    /// Top interest clusters extracted from reasoning (e.g., "rust", "ai", "debugging").
    pub interest_clusters: Vec<String>,
}

/// Response to InferenceExplanation query: last inference cycle with context.
#[derive(Debug, Clone)]
pub struct InferenceExplanationResponse {
    /// The user prompt or task that triggered the inference.
    pub request_context: String,
    /// The full response Sena generated.
    pub response_text: String,
    /// Working memory chunks that were in context during inference.
    pub working_memory_context: Vec<MemoryChunk>,
    /// Number of reasoning rounds executed.
    pub rounds_completed: usize,
}

/// Top-level transparency event enum.
#[derive(Debug, Clone)]
pub enum TransparencyEvent {
    QueryRequested(TransparencyQuery),
    ObservationResponded(ObservationResponse),
    MemoryResponded(MemoryResponse),
    InferenceExplanationResponded(InferenceExplanationResponse),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparency_events_clone_and_send() {
        let observation = TransparencyQuery::CurrentObservation;
        let _cloned = observation.clone();

        let memory = TransparencyQuery::UserMemory;
        let _cloned = memory.clone();

        let explanation = TransparencyQuery::InferenceExplanation;
        let _cloned = explanation.clone();
    }

    #[test]
    fn soul_summary_for_transparency_redacts_raw_data() {
        let summary = SoulSummaryForTransparency {
            inference_cycle_count: 42,
            work_patterns: vec!["morning_coder".into()],
            tool_preferences: vec!["vscode".into()],
            interest_clusters: vec!["rust".into()],
        };
        // Verify: only aggregates are present, no raw identity data
        assert_eq!(summary.inference_cycle_count, 42);
        assert_eq!(summary.work_patterns.len(), 1);
        assert_eq!(summary.tool_preferences.len(), 1);
        assert_eq!(summary.interest_clusters.len(), 1);
    }

    fn assert_send_static<T: Send + 'static>() {}

    #[test]
    fn all_transparency_types_are_send_and_static() {
        assert_send_static::<TransparencyQuery>();
        assert_send_static::<ObservationResponse>();
        assert_send_static::<MemoryResponse>();
        assert_send_static::<InferenceExplanationResponse>();
        assert_send_static::<TransparencyEvent>();
        assert_send_static::<SoulSummaryForTransparency>();
    }
}
