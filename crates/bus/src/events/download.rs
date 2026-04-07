//! Download event types for model download pipeline.
//!
//! These events are emitted during any Sena model download operation
//! (embedding models, future model auto-downloads). Not speech-specific.

/// Events emitted during model download operations.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// Model download has been initiated.
    Started {
        /// Human-readable model name.
        model_name: String,
        /// Total file size in bytes.
        total_bytes: u64,
        /// Request ID for correlation.
        request_id: u64,
    },
    /// Download progress update.
    Progress {
        /// Human-readable model name.
        model_name: String,
        /// Bytes downloaded so far.
        bytes_downloaded: u64,
        /// Total file size in bytes.
        total_bytes: u64,
        /// Request ID for correlation.
        request_id: u64,
    },
    /// Download completed successfully.
    Completed {
        /// Human-readable model name.
        model_name: String,
        /// Absolute path to the cached model file.
        cached_path: String,
        /// Request ID for correlation.
        request_id: u64,
    },
    /// Download failed.
    Failed {
        /// Human-readable model name.
        model_name: String,
        /// Failure reason.
        reason: String,
        /// Request ID for correlation.
        request_id: u64,
    },
}
