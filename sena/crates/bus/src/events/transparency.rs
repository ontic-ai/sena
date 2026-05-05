//! Transparency query events — PR Principle P7: "Earn trust through transparency"
//!
//! Users can ask Sena fundamental questions about its current state and reasoning:
//! 1. "What are you observing right now?" — Current context signals
//! 2. "What do you remember about me?" — Accumulated personalization state
//! 3. "Why did you say that?" — Reasoning chain for a specific thought
//!
//! These events define the protocol between CLI/IPC and the responder actors.

use crate::events::ctp::ContextSnapshot;
use crate::events::memory::MemoryChunk;
use serde::{Deserialize, Serialize};

/// User query for transparency: ask Sena about its current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransparencyQuery {
    /// "What are you observing right now?"
    CurrentObservation,
    /// "What do you remember about me?"
    UserMemory,
    /// "Why did you say that?" — Reasoning for a specific thought/interaction
    ReasoningChain {
        /// ID of the thought event to explain.
        thought_id: String,
    },
}

/// Transparency response/events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransparencyEvent {
    /// User requested transparency information.
    QueryRequested(TransparencyQuery),
    /// Response to a transparency query.
    QueryResponse {
        /// The query this is a response to.
        query: TransparencyQuery,
        /// The result of the query.
        result: Box<TransparencyResult>,
    },
}

/// Result of a transparency query.
///
/// This is the canonical unified response envelope. All transparency responses
/// use this structure, with richer structured payloads inside each variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransparencyResult {
    /// Observation result: the current context snapshot.
    Observation(ObservationResponse),
    /// Memory result: soul summary + recent memory chunks.
    Memory(MemoryResponse),
    /// Reasoning result: the chain of thought for a specific inference.
    Reasoning(ReasoningResponse),
}

/// Response to CurrentObservation query: current context snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationResponse {
    /// The current context snapshot assembled by CTP.
    pub snapshot: Box<ContextSnapshot>,
}

/// Response to UserMemory query: soul summary + recent memory chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryResponse {
    /// High-level summary of Sena's understanding of the user.
    pub soul_summary: SoulSummary,
    /// Recent and important memory chunks, ranked by relevance.
    pub memory_chunks: Vec<MemoryChunk>,
}

/// A redacted view of soul/identity state for transparency purposes.
///
/// Only includes high-level aggregates, never raw identity data or PII.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulSummary {
    /// Number of inference cycles completed.
    pub inference_cycle_count: usize,
    /// Top work patterns observed (e.g., "morning_coder", "late_night_writer").
    pub work_patterns: Vec<String>,
    /// Top tool preferences (e.g., "vscode", "cargo", "chrome").
    pub tool_preferences: Vec<String>,
    /// Top interest clusters extracted from reasoning (e.g., "rust", "ai", "debugging").
    pub interest_clusters: Vec<String>,
}

/// Response to ReasoningChain query: last inference cycle with context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningResponse {
    /// Causal ID of the inference this explains.
    pub causal_id: u64,
    /// The source of the inference (user voice, user text, proactive CTP, etc.).
    pub source_description: String,
    /// Token count of the inference response.
    pub token_count: usize,
    /// Preview of the response text (truncated for display).
    pub response_preview: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::platform::{KeystrokeCadence, WindowContext};
    use std::time::{Duration, Instant};

    #[test]
    fn transparency_observation_result_serializes() {
        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("workspace".to_string()),
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: Vec::new(),
            clipboard_digest: Some("digest".to_string()),
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 42.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(1),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(5),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: None,
        };

        let event = TransparencyEvent::QueryResponse {
            query: TransparencyQuery::CurrentObservation,
            result: Box::new(TransparencyResult::Observation(ObservationResponse {
                snapshot: Box::new(snapshot),
            })),
        };

        let json = serde_json::to_string(&event).expect("transparency events should serialize");
        assert!(json.contains("CurrentObservation"));
        assert!(json.contains("Observation"));
    }

    #[test]
    fn transparency_memory_result_serializes() {
        let response = MemoryResponse {
            soul_summary: SoulSummary {
                inference_cycle_count: 42,
                work_patterns: vec!["morning_coder".to_string()],
                tool_preferences: vec!["vscode".to_string(), "cargo".to_string()],
                interest_clusters: vec!["rust".to_string(), "ai".to_string()],
            },
            memory_chunks: vec![MemoryChunk {
                content: "test memory".to_string(),
                score: 0.95,
                age_seconds: 100,
            }],
        };

        let event = TransparencyEvent::QueryResponse {
            query: TransparencyQuery::UserMemory,
            result: Box::new(TransparencyResult::Memory(response)),
        };

        let json = serde_json::to_string(&event).expect("transparency events should serialize");
        assert!(json.contains("UserMemory"));
        assert!(json.contains("Memory"));
        assert!(json.contains("morning_coder"));
    }

    #[test]
    fn transparency_reasoning_result_serializes() {
        let response = ReasoningResponse {
            causal_id: 12345,
            source_description: "user voice input".to_string(),
            token_count: 256,
            response_preview: "This is a preview of the response...".to_string(),
        };

        let event = TransparencyEvent::QueryResponse {
            query: TransparencyQuery::ReasoningChain {
                thought_id: "latest".to_string(),
            },
            result: Box::new(TransparencyResult::Reasoning(response)),
        };

        let json = serde_json::to_string(&event).expect("transparency events should serialize");
        assert!(json.contains("ReasoningChain"));
        assert!(json.contains("Reasoning"));
        assert!(json.contains("12345"));
    }

    #[test]
    fn soul_summary_redacts_raw_data() {
        let summary = SoulSummary {
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
        assert_send_static::<TransparencyEvent>();
        assert_send_static::<TransparencyResult>();
        assert_send_static::<ObservationResponse>();
        assert_send_static::<MemoryResponse>();
        assert_send_static::<ReasoningResponse>();
        assert_send_static::<SoulSummary>();
    }
}
