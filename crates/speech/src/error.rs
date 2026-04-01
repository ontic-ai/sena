//! Speech subsystem errors.

/// Speech subsystem errors.
#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    /// STT initialization failed.
    #[error("STT init failed: {0}")]
    SttInitFailed(String),

    /// TTS initialization failed.
    #[error("TTS init failed: {0}")]
    TtsInitFailed(String),

    /// Audio capture failed.
    #[error("audio capture failed: {0}")]
    AudioCaptureFailed(String),

    /// Audio playback failed.
    #[error("audio playback failed: {0}")]
    AudioPlaybackFailed(String),

    /// Transcription processing failed.
    #[error("transcription failed: {0}")]
    TranscriptionFailed(String),

    /// Speech generation failed.
    #[error("speech generation failed: {0}")]
    SpeechGenerationFailed(String),

    /// Channel operation failed.
    #[error("channel closed: {0}")]
    ChannelClosed(String),
}
