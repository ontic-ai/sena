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

    /// Model download failed.
    #[error("model download failed: {0}")]
    DownloadFailed(String),

    /// Checksum verification failed.
    #[error("checksum verification failed: {0}")]
    ChecksumVerificationFailed(String),

    /// Checksum mismatch after download.
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// Expected SHA-256 checksum.
        expected: String,
        /// Actual SHA-256 checksum.
        actual: String,
    },

    /// Model not found in manifest.
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// STT worker binary not found.
    #[error("stt-worker binary not found at: {0}")]
    WorkerNotFound(std::path::PathBuf),

    /// STT worker process failed to spawn.
    #[error("worker spawn failed: {0}")]
    WorkerSpawnFailed(String),

    /// STT worker process crashed.
    #[error("worker crashed with exit code: {0}")]
    WorkerCrashed(i32),

    /// Communication with STT worker failed.
    #[error("worker pipe error: {0}")]
    WorkerPipeError(String),
}
