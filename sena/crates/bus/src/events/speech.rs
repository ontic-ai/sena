//! Speech events — voice input, transcription, voice output.

use crate::causal::CausalId;

/// Speech-related events.
#[derive(Debug, Clone)]
pub enum SpeechEvent {
    /// Voice input detected from microphone.
    VoiceInputDetected {
        /// Raw audio bytes.
        audio_bytes: Vec<u8>,
        /// Duration of the audio clip in milliseconds.
        duration_ms: u64,
    },

    /// Transcription completed successfully.
    TranscriptionCompleted {
        /// Transcribed text.
        text: String,
        /// Confidence score [0.0, 1.0].
        confidence: f32,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Transcription failed.
    TranscriptionFailed {
        /// Failure reason.
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Request to speak text aloud.
    SpeakRequested {
        /// Text to speak.
        text: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Speech output completed successfully.
    SpeechOutputCompleted {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Speech generation or playback failed.
    SpeechFailed {
        /// Failure reason.
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Wakeword detected in audio stream.
    WakewordDetected {
        /// Confidence score [0.0, 1.0].
        confidence: f32,
    },
}

impl SpeechEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::TranscriptionCompleted { causal_id, .. }
            | Self::TranscriptionFailed { causal_id, .. }
            | Self::SpeakRequested { causal_id, .. }
            | Self::SpeechOutputCompleted { causal_id, .. }
            | Self::SpeechFailed { causal_id, .. } => Some(*causal_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_event_causal_id_extraction() {
        let cid = CausalId::new();
        let event = SpeechEvent::TranscriptionCompleted {
            text: "test".to_string(),
            confidence: 0.9,
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn wakeword_event_has_no_causal_id() {
        let event = SpeechEvent::WakewordDetected { confidence: 0.8 };
        assert_eq!(event.causal_id(), None);
    }
}
