//! Inference-layer events: model discovery, requests, responses.

use crate::causal::CausalId;

/// Priority level for inference requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Highest priority — processed first.
    High = 2,
    /// Normal priority — default.
    Normal = 1,
    /// Low priority — background tasks.
    Low = 0,
}

/// Source of an inference request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceSource {
    /// User spoke — transcription completed and triggered inference.
    UserVoice,
    /// User typed — text input from CLI or shell.
    UserText,
    /// Proactive CTP trigger.
    ProactiveCTP,
    /// Iterative reasoning.
    Iterative,
}

/// Origin point of an inference failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceFailureOrigin {
    /// Failure originated from a user-initiated request.
    UserRequest,
    /// Failure during embedding generation (internal memory operation).
    EmbeddingInternal,
    /// Failure during fact extraction (internal memory operation).
    ExtractionInternal,
    /// Failure during proactive CTP-triggered inference.
    ProactiveCTP,
}

/// Inference-layer events.
#[derive(Debug, Clone)]
pub enum InferenceEvent {
    /// Inference request submitted.
    InferenceRequested {
        prompt: String,
        priority: Priority,
        source: InferenceSource,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Inference response produced.
    InferenceCompleted {
        text: String,
        token_count: usize,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Inference request failed (legacy variant for backward compatibility).
    InferenceFailed {
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Inference request failed with origin information.
    InferenceFailedWithOrigin {
        origin: InferenceFailureOrigin,
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// A single token produced during streaming inference.
    InferenceTokenGenerated {
        token: String,
        sequence_number: u64,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// A complete sentence detected during streaming inference.
    InferenceSentenceReady {
        text: String,
        /// Sentence index for ordering in TTS queue.
        sentence_index: u32,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Streaming inference completed.
    InferenceStreamCompleted {
        full_text: String,
        source: InferenceSource,
        token_count: usize,
        /// Confidence score for the response (0.0-1.0). None for batch path.
        confidence: Option<f32>,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Exploration phase of inference completed (multi-step reasoning).
    InferenceExplorationCompleted {
        steps_completed: usize,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Embedding request submitted (internal memory operation).
    EmbedRequested { text: String, request_id: u64 },

    /// Embedding response produced.
    EmbedCompleted { vector: Vec<f32>, request_id: u64 },

    /// Embedding request failed.
    EmbedFailed { request_id: u64, reason: String },

    /// Fact extraction request submitted (internal memory operation).
    ExtractionRequested { text: String, request_id: u64 },

    /// Fact extraction response produced.
    ExtractionCompleted { facts: Vec<String>, request_id: u64 },
}

impl InferenceEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::InferenceRequested { causal_id, .. }
            | Self::InferenceCompleted { causal_id, .. }
            | Self::InferenceFailed { causal_id, .. }
            | Self::InferenceFailedWithOrigin { causal_id, .. }
            | Self::InferenceTokenGenerated { causal_id, .. }
            | Self::InferenceSentenceReady { causal_id, .. }
            | Self::InferenceStreamCompleted { causal_id, .. }
            | Self::InferenceExplorationCompleted { causal_id, .. } => Some(*causal_id),
            Self::EmbedRequested { .. }
            | Self::EmbedCompleted { .. }
            | Self::EmbedFailed { .. }
            | Self::ExtractionRequested { .. }
            | Self::ExtractionCompleted { .. } => None,
        }
    }

    /// Create an InferenceFailed event without specifying origin (backward compatible).
    pub fn failed(reason: String, causal_id: CausalId) -> Self {
        Self::InferenceFailed { reason, causal_id }
    }

    /// Create an InferenceFailed event with the specified origin.
    pub fn failed_with_origin(
        origin: InferenceFailureOrigin,
        reason: String,
        causal_id: CausalId,
    ) -> Self {
        Self::InferenceFailedWithOrigin {
            origin,
            reason,
            causal_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_event_causal_id_extraction() {
        let cid = CausalId::new();
        let event = InferenceEvent::InferenceRequested {
            prompt: "test".to_string(),
            priority: Priority::Normal,
            source: InferenceSource::UserText,
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
    }

    #[test]
    fn inference_failure_origin_variants() {
        let origins = [
            InferenceFailureOrigin::UserRequest,
            InferenceFailureOrigin::EmbeddingInternal,
            InferenceFailureOrigin::ExtractionInternal,
            InferenceFailureOrigin::ProactiveCTP,
        ];
        assert_eq!(origins.len(), 4);
    }

    #[test]
    fn inference_sentence_ready_constructs() {
        let cid = CausalId::new();
        let event = InferenceEvent::InferenceSentenceReady {
            text: "Hello world.".to_string(),
            sentence_index: 0,
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn inference_stream_completed_constructs() {
        let cid = CausalId::new();
        let event = InferenceEvent::InferenceStreamCompleted {
            full_text: "Hello world.".to_string(),
            source: InferenceSource::UserText,
            token_count: 100,
            confidence: Some(0.95),
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn inference_exploration_completed_constructs() {
        let cid = CausalId::new();
        let event = InferenceEvent::InferenceExplorationCompleted {
            steps_completed: 5,
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }
}
