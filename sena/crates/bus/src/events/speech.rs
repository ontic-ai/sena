//! Speech events — voice input, transcription, voice output.

use crate::causal::CausalId;

/// A single transcribed word with timing and confidence metadata.
#[derive(Debug, Clone)]
pub struct TranscribedWord {
    /// The word text.
    pub text: String,
    /// Confidence score [0.0, 1.0].
    pub confidence: f32,
    /// Start time offset in milliseconds from audio start.
    pub start_ms: u64,
    /// End time offset in milliseconds from audio start.
    pub end_ms: u64,
}

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

    /// A single word was transcribed and is ready.
    TranscriptionWordReady {
        word: TranscribedWord,
        /// Causal chain ID.
        causal_id: CausalId,
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

    /// STT model loaded successfully.
    SttModelLoaded { model_name: String },

    /// STT model load failed.
    SttModelLoadFailed { model_name: String, reason: String },

    /// STT is now listening for input.
    SttListening,

    /// STT stopped listening.
    SttStopped,

    /// STT cancelled by user or system.
    SttCancelled,

    /// Request to speak text aloud.
    SpeakRequested {
        /// Text to speak.
        text: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Speech synthesis started.
    SpeakingStarted {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Speech output completed successfully.
    SpeakingCompleted {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Speech was interrupted before completion.
    SpeakingInterrupted {
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

    /// Transcription below confidence threshold — not routed to inference.
    LowConfidenceTranscription {
        /// The transcribed text (for debugging/observability).
        text: String,
        /// Confidence score that triggered this event.
        confidence: f32,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Wakeword detection suppressed (e.g., during TTS playback).
    WakewordSuppressed {
        /// Reason for suppression.
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Wakeword detection resumed after suppression.
    WakewordResumed {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Listen mode requested (continuous transcription session).
    ListenModeRequested {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Listen mode transcription event (partial or final).
    ListenModeTranscription {
        /// Transcribed text.
        text: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Listen mode stop requested.
    ListenModeStopRequested {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Listen mode stopped.
    ListenModeStopped {
        /// Causal chain ID.
        causal_id: CausalId,
        /// Raw transcript accumulated during the listen session, if any.
        transcript: Option<String>,
    },

    /// Listen mode transcript finalized for display after cleanup.
    ListenModeTranscriptFinalized {
        /// Finalized transcript text.
        text: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Audio device changed (hot-swap event).
    AudioDeviceChanged {
        /// New device name.
        device_name: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Speech output completed (legacy event).
    SpeechOutputCompleted { causal_id: CausalId },
}

impl SpeechEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::TranscriptionWordReady { causal_id, .. }
            | Self::TranscriptionCompleted { causal_id, .. }
            | Self::TranscriptionFailed { causal_id, .. }
            | Self::LowConfidenceTranscription { causal_id, .. }
            | Self::SpeakRequested { causal_id, .. }
            | Self::SpeakingStarted { causal_id }
            | Self::SpeakingCompleted { causal_id }
            | Self::SpeakingInterrupted { causal_id }
            | Self::SpeechOutputCompleted { causal_id }
            | Self::SpeechFailed { causal_id, .. }
            | Self::WakewordSuppressed { causal_id, .. }
            | Self::WakewordResumed { causal_id }
            | Self::ListenModeRequested { causal_id }
            | Self::ListenModeTranscription { causal_id, .. }
            | Self::ListenModeStopRequested { causal_id }
            | Self::ListenModeStopped { causal_id, .. }
            | Self::ListenModeTranscriptFinalized { causal_id, .. }
            | Self::AudioDeviceChanged { causal_id, .. } => Some(*causal_id),
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

    #[test]
    fn transcribed_word_constructs() {
        let word = TranscribedWord {
            text: "hello".to_string(),
            confidence: 0.95,
            start_ms: 100,
            end_ms: 300,
        };
        assert_eq!(word.text, "hello");
        assert_eq!(word.start_ms, 100);
        assert_eq!(word.end_ms, 300);
    }

    #[test]
    fn transcription_word_ready_constructs() {
        let cid = CausalId::new();
        let word = TranscribedWord {
            text: "test".to_string(),
            confidence: 0.9,
            start_ms: 0,
            end_ms: 200,
        };
        let event = SpeechEvent::TranscriptionWordReady {
            word,
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn stt_model_events_construct() {
        let loaded = SpeechEvent::SttModelLoaded {
            model_name: "whisper-base".to_string(),
        };
        let failed = SpeechEvent::SttModelLoadFailed {
            model_name: "whisper-large".to_string(),
            reason: "out of memory".to_string(),
        };
        assert!(matches!(loaded, SpeechEvent::SttModelLoaded { .. }));
        assert!(matches!(failed, SpeechEvent::SttModelLoadFailed { .. }));
    }

    #[test]
    fn stt_lifecycle_events_construct() {
        let listening = SpeechEvent::SttListening;
        let stopped = SpeechEvent::SttStopped;
        let cancelled = SpeechEvent::SttCancelled;
        assert!(matches!(listening, SpeechEvent::SttListening));
        assert!(matches!(stopped, SpeechEvent::SttStopped));
        assert!(matches!(cancelled, SpeechEvent::SttCancelled));
    }

    #[test]
    fn speaking_lifecycle_events_construct() {
        let cid = CausalId::new();
        let started = SpeechEvent::SpeakingStarted { causal_id: cid };
        let completed = SpeechEvent::SpeakingCompleted { causal_id: cid };
        let interrupted = SpeechEvent::SpeakingInterrupted { causal_id: cid };
        assert_eq!(started.causal_id(), Some(cid));
        assert_eq!(completed.causal_id(), Some(cid));
        assert_eq!(interrupted.causal_id(), Some(cid));
    }
}
