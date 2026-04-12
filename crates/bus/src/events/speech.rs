//! Speech events — voice input, transcription, voice output.

/// Word-level timing and confidence data from STT transcription.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscribedWord {
    /// The transcribed word text.
    pub text: String,
    /// Confidence score for this word [0.0, 1.0].
    pub confidence: f32,
    /// Start time in milliseconds from the beginning of the audio.
    pub start_ms: u32,
    /// End time in milliseconds from the beginning of the audio.
    pub end_ms: u32,
}

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
        /// Overall confidence score [0.0, 1.0] (kept for backward compat).
        confidence: f32,
        /// Request ID for correlation.
        request_id: u64,
        /// Word-level transcription with timing and confidence data.
        words: Vec<TranscribedWord>,
        /// Average confidence across all words.
        average_confidence: f32,
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

    /// Streaming STT word emitted during transcription.
    /// Emitted per word as whisper-rs processes the audio.
    TranscriptionWordReady {
        /// The transcribed word.
        word: String,
        /// Confidence score for this word [0.0, 1.0].
        confidence: f32,
        /// Sequence number for ordering (starts at 0 for each request).
        sequence: u32,
        /// Request ID for correlation.
        request_id: u64,
    },

    /// STT model loaded successfully.
    SttModelLoaded {
        /// Name of the loaded model.
        model_name: String,
        /// Backend identifier (e.g., "whisper-rs").
        backend: String,
    },

    /// STT model load failed.
    SttModelLoadFailed {
        /// Failure reason.
        reason: String,
    },

    /// STT is actively listening for voice input.
    SttListening,

    /// STT stopped listening.
    SttStopped,

    /// STT transcription was cancelled (e.g., user typed text while voice was active).
    SttCancelled {
        /// Request ID that was cancelled.
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
            words: vec![],
            average_confidence: 0.95,
        };
        let cloned = event.clone();
        if let SpeechEvent::TranscriptionCompleted {
            text,
            confidence,
            request_id,
            ..
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

    // Verify Send + 'static trait bounds are satisfied at compile time
    fn _assert_send_static<T: Send + 'static>() {}

    #[test]
    fn speech_event_is_send_and_static() {
        _assert_send_static::<SpeechEvent>();
    }
}
