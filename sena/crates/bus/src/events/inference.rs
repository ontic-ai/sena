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

    /// Inference request failed.
    InferenceFailed {
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
}

impl InferenceEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::InferenceRequested { causal_id, .. }
            | Self::InferenceCompleted { causal_id, .. }
            | Self::InferenceFailed { causal_id, .. }
            | Self::InferenceTokenGenerated { causal_id, .. } => Some(*causal_id),
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
}
