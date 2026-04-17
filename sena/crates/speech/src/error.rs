//! Error types for the speech subsystem.

/// STT backend errors.
#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("audio processing error: {0}")]
    AudioProcessing(String),

    #[error("backend initialization failed: {0}")]
    InitializationFailed(String),

    #[error("transcription failed: {0}")]
    TranscriptionFailed(String),

    #[error("invalid audio format: {0}")]
    InvalidAudioFormat(String),

    #[error("backend error: {0}")]
    BackendError(String),
}

/// TTS backend errors.
#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("synthesis failed: {0}")]
    SynthesisFailed(String),

    #[error("backend initialization failed: {0}")]
    InitializationFailed(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("backend error: {0}")]
    BackendError(String),
}

/// Speech actor errors.
#[derive(Debug, thiserror::Error)]
pub enum SpeechActorError {
    #[error("STT error: {0}")]
    Stt(#[from] SttError),

    #[error("TTS error: {0}")]
    Tts(#[from] TtsError),

    #[error("audio device error: {0}")]
    AudioDevice(String),

    #[error("bus error: {0}")]
    Bus(String),

    #[error("shutdown requested")]
    Shutdown,
}
