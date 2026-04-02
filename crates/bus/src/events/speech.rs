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

    /// Model download initiated.
    ModelDownloadStarted {
        /// Name of the model being downloaded.
        model_name: String,
        /// Total size in bytes.
        total_bytes: u64,
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Model download progress update.
    ModelDownloadProgress {
        /// Name of the model being downloaded.
        model_name: String,
        /// Bytes downloaded so far.
        bytes_downloaded: u64,
        /// Total size in bytes.
        total_bytes: u64,
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Model download completed successfully.
    ModelDownloadCompleted {
        /// Name of the model that was downloaded.
        model_name: String,
        /// Path to the cached model file.
        cached_path: String,
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Model download failed.
    ModelDownloadFailed {
        /// Name of the model that failed to download.
        model_name: String,
        /// Failure reason.
        reason: String,
        /// Request ID for correlation.
        request_id: u64,
    },

    /// Wakeword detected in audio stream.
    WakewordDetected {
        /// Confidence score [0.0, 1.0].
        confidence: f32,
    },

    /// Speech onboarding process started.
    SpeechOnboardingStarted,

    /// Speech onboarding completed successfully.
    SpeechOnboardingCompleted {
        /// List of models successfully downloaded.
        models_downloaded: Vec<String>,
    },

    /// Speech onboarding failed.
    SpeechOnboardingFailed {
        /// Failure reason.
        reason: String,
        /// Whether the failure is recoverable (user can retry).
        recoverable: bool,
    },

    /// Wakeword detection suppressed (e.g., while TTS is playing).
    /// The wakeword actor will not emit WakewordDetected while suppressed.
    WakewordSuppressed {
        /// Human-readable reason for suppression.
        reason: String,
    },

    /// Wakeword detection resumed after suppression.
    WakewordResumed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_input_detected_constructs_and_clones() {
        let event = SpeechEvent::VoiceInputDetected {
            audio_bytes: vec![1, 2, 3],
            duration_ms: 1000,
        };
        let cloned = event.clone();
        if let SpeechEvent::VoiceInputDetected {
            audio_bytes,
            duration_ms,
        } = cloned
        {
            assert_eq!(audio_bytes, vec![1, 2, 3]);
            assert_eq!(duration_ms, 1000);
        } else {
            panic!("Expected VoiceInputDetected variant");
        }
    }

    #[test]
    fn transcription_completed_constructs_and_clones() {
        let event = SpeechEvent::TranscriptionCompleted {
            text: "hello".to_string(),
            confidence: 0.95,
            request_id: 42,
        };
        let cloned = event.clone();
        if let SpeechEvent::TranscriptionCompleted {
            text,
            confidence,
            request_id,
        } = cloned
        {
            assert_eq!(text, "hello");
            assert_eq!(confidence, 0.95);
            assert_eq!(request_id, 42);
        } else {
            panic!("Expected TranscriptionCompleted variant");
        }
    }

    #[test]
    fn model_download_started_constructs_and_clones() {
        let event = SpeechEvent::ModelDownloadStarted {
            model_name: "whisper-base".to_string(),
            total_bytes: 1024,
            request_id: 1,
        };
        let cloned = event.clone();
        if let SpeechEvent::ModelDownloadStarted {
            model_name,
            total_bytes,
            request_id,
        } = cloned
        {
            assert_eq!(model_name, "whisper-base");
            assert_eq!(total_bytes, 1024);
            assert_eq!(request_id, 1);
        } else {
            panic!("Expected ModelDownloadStarted variant");
        }
    }

    #[test]
    fn model_download_progress_constructs_and_clones() {
        let event = SpeechEvent::ModelDownloadProgress {
            model_name: "whisper-base".to_string(),
            bytes_downloaded: 512,
            total_bytes: 1024,
            request_id: 1,
        };
        let cloned = event.clone();
        if let SpeechEvent::ModelDownloadProgress {
            model_name,
            bytes_downloaded,
            total_bytes,
            request_id,
        } = cloned
        {
            assert_eq!(model_name, "whisper-base");
            assert_eq!(bytes_downloaded, 512);
            assert_eq!(total_bytes, 1024);
            assert_eq!(request_id, 1);
        } else {
            panic!("Expected ModelDownloadProgress variant");
        }
    }

    #[test]
    fn model_download_completed_constructs_and_clones() {
        let event = SpeechEvent::ModelDownloadCompleted {
            model_name: "whisper-base".to_string(),
            cached_path: "/path/to/model.bin".to_string(),
            request_id: 1,
        };
        let cloned = event.clone();
        if let SpeechEvent::ModelDownloadCompleted {
            model_name,
            cached_path,
            request_id,
        } = cloned
        {
            assert_eq!(model_name, "whisper-base");
            assert_eq!(cached_path, "/path/to/model.bin");
            assert_eq!(request_id, 1);
        } else {
            panic!("Expected ModelDownloadCompleted variant");
        }
    }

    #[test]
    fn model_download_failed_constructs_and_clones() {
        let event = SpeechEvent::ModelDownloadFailed {
            model_name: "whisper-base".to_string(),
            reason: "network error".to_string(),
            request_id: 1,
        };
        let cloned = event.clone();
        if let SpeechEvent::ModelDownloadFailed {
            model_name,
            reason,
            request_id,
        } = cloned
        {
            assert_eq!(model_name, "whisper-base");
            assert_eq!(reason, "network error");
            assert_eq!(request_id, 1);
        } else {
            panic!("Expected ModelDownloadFailed variant");
        }
    }

    #[test]
    fn wakeword_detected_constructs_and_clones() {
        let event = SpeechEvent::WakewordDetected { confidence: 0.88 };
        let cloned = event.clone();
        if let SpeechEvent::WakewordDetected { confidence } = cloned {
            assert_eq!(confidence, 0.88);
        } else {
            panic!("Expected WakewordDetected variant");
        }
    }

    #[test]
    fn speech_onboarding_started_constructs_and_clones() {
        let event = SpeechEvent::SpeechOnboardingStarted;
        let cloned = event.clone();
        assert!(matches!(cloned, SpeechEvent::SpeechOnboardingStarted));
    }

    #[test]
    fn speech_onboarding_completed_constructs_and_clones() {
        let event = SpeechEvent::SpeechOnboardingCompleted {
            models_downloaded: vec!["whisper".to_string(), "piper".to_string()],
        };
        let cloned = event.clone();
        if let SpeechEvent::SpeechOnboardingCompleted { models_downloaded } = cloned {
            assert_eq!(models_downloaded, vec!["whisper", "piper"]);
        } else {
            panic!("Expected SpeechOnboardingCompleted variant");
        }
    }

    #[test]
    fn speech_onboarding_failed_constructs_and_clones() {
        let event = SpeechEvent::SpeechOnboardingFailed {
            reason: "disk full".to_string(),
            recoverable: true,
        };
        let cloned = event.clone();
        if let SpeechEvent::SpeechOnboardingFailed {
            reason,
            recoverable,
        } = cloned
        {
            assert_eq!(reason, "disk full");
            assert_eq!(recoverable, true);
        } else {
            panic!("Expected SpeechOnboardingFailed variant");
        }
    }

    // Verify Send + 'static trait bounds are satisfied at compile time
    fn _assert_send_static<T: Send + 'static>() {}

    #[test]
    fn speech_event_is_send_and_static() {
        _assert_send_static::<SpeechEvent>();
    }
}
