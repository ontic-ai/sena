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

    /// Transcription completed but confidence was below acceptable threshold.
    /// User should be informed that speech was detected but not understood clearly.
    LowConfidenceTranscription {
        /// Confidence score that triggered this event.
        confidence: f32,
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

    /// User requested continuous listen mode (e.g., via `/listen` CLI command).
    ListenModeRequested {
        /// Session ID for correlating start/stop and transcription events.
        session_id: u64,
    },

    /// Incremental transcription result from continuous listen mode.
    ///
    /// May be emitted multiple times per utterance:
    /// - `is_final = false`: partial, may be superseded by the next emission.
    /// - `is_final = true`: confirmed utterance after silence detected.
    ListenModeTranscription {
        /// Transcribed text.
        text: String,
        /// True when silence detected and this utterance is complete.
        is_final: bool,
        /// Confidence score [0.0, 1.0].
        confidence: f32,
        /// Session ID from the originating `ListenModeRequested`.
        session_id: u64,
    },

    /// Request to stop an active continuous listen session.
    ListenModeStopRequested {
        /// Session ID that should be stopped.
        session_id: u64,
    },

    /// Continuous listen session stopped cleanly.
    ListenModeStopped {
        /// Session ID that was stopped.
        session_id: u64,
    },

    /// User or system requests STT backend change.
    SttBackendSwitchRequested {
        /// Target backend: "whisper", "sherpa", "parakeet"
        backend: String,
    },

    /// STT actor confirms backend switch completed.
    SttBackendSwitchCompleted {
        /// Backend that is now active.
        backend: String,
    },

    /// STT actor reports backend switch failed.
    SttBackendSwitchFailed {
        /// Backend that was requested.
        backend: String,
        /// Failure reason.
        reason: String,
    },

    /// STT actor emits telemetry after each transcription.
    SttTelemetryUpdate {
        /// Backend used for this transcription.
        backend: String,
        /// VRAM usage in MB (None if not measurable).
        vram_mb: Option<f64>,
        /// Time from audio end to text ready in milliseconds.
        latency_ms: f64,
        /// Average word confidence [0.0, 1.0].
        avg_confidence: f64,
        /// Request ID for correlation.
        request_id: u64,
    },
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

    #[test]
    fn stt_backend_switch_requested_constructs_and_clones() {
        let event = SpeechEvent::SttBackendSwitchRequested {
            backend: "sherpa".to_string(),
        };
        let cloned = event.clone();
        if let SpeechEvent::SttBackendSwitchRequested { backend } = cloned {
            assert_eq!(backend, "sherpa");
        } else {
            panic!("Expected SttBackendSwitchRequested variant");
        }
    }

    #[test]
    fn stt_backend_switch_completed_constructs_and_clones() {
        let event = SpeechEvent::SttBackendSwitchCompleted {
            backend: "whisper".to_string(),
        };
        let cloned = event.clone();
        if let SpeechEvent::SttBackendSwitchCompleted { backend } = cloned {
            assert_eq!(backend, "whisper");
        } else {
            panic!("Expected SttBackendSwitchCompleted variant");
        }
    }

    #[test]
    fn stt_backend_switch_failed_constructs_and_clones() {
        let event = SpeechEvent::SttBackendSwitchFailed {
            backend: "parakeet".to_string(),
            reason: "model not found".to_string(),
        };
        let cloned = event.clone();
        if let SpeechEvent::SttBackendSwitchFailed { backend, reason } = cloned {
            assert_eq!(backend, "parakeet");
            assert_eq!(reason, "model not found");
        } else {
            panic!("Expected SttBackendSwitchFailed variant");
        }
    }

    #[test]
    fn stt_telemetry_update_constructs_and_clones() {
        let event = SpeechEvent::SttTelemetryUpdate {
            backend: "whisper".to_string(),
            vram_mb: Some(256.0),
            latency_ms: 120.5,
            avg_confidence: 0.92,
            request_id: 42,
        };
        let cloned = event.clone();
        if let SpeechEvent::SttTelemetryUpdate {
            backend,
            vram_mb,
            latency_ms,
            avg_confidence,
            request_id,
        } = cloned
        {
            assert_eq!(backend, "whisper");
            assert_eq!(vram_mb, Some(256.0));
            assert_eq!(latency_ms, 120.5);
            assert_eq!(avg_confidence, 0.92);
            assert_eq!(request_id, 42);
        } else {
            panic!("Expected SttTelemetryUpdate variant");
        }
    }

    // Verify Send + 'static trait bounds are satisfied at compile time
    fn _assert_send_static<T: Send + 'static>() {}

    #[test]
    fn speech_event_is_send_and_static() {
        _assert_send_static::<SpeechEvent>();
    }
}
