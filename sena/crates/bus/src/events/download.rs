//! Download event types for model download pipeline.
//!
//! These events are emitted during any Sena model download operation
//! (embedding models, speech models). Not speech-specific.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_events_are_cloneable() {
        let event = DownloadEvent::Started {
            model_name: "test-model".to_string(),
            total_bytes: 1000,
            request_id: 42,
        };
        let cloned = event.clone();
        assert!(matches!(cloned, DownloadEvent::Started { .. }));
    }
}
