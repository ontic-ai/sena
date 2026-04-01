//! Speech events — voice input, transcription, voice output.

/// Speech-related events.
#[derive(Debug, Clone)]
pub enum SpeechEvent {
    /// Voice input detected from microphone.
    VoiceInputDetected {
        /// Raw audio bytes (format depends on STT backend requirements).
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
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Transcription failed.
    TranscriptionFailed {
        /// Failure reason.
        reason: String,
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Request to speak text aloud.
    SpeakRequested {
        /// Text to speak.
        text: String,
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Speech output completed successfully.
    SpeechOutputCompleted {
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Speech generation or playback failed.
    SpeechFailed {
        /// Failure reason.
        reason: String,
        /// Request ID for correlation.
        request_id: u64,
    },
}
