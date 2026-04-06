//! Inference-layer events: model discovery, requests, responses.

// Re-export ModelInfo and Quantization from the infer crate
pub use infer::{ModelInfo, Quantization};

/// Priority level for inference requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Highest priority — processed first.
    High = 2,
    /// Normal priority — default.
    Normal = 1,
    /// Low priority — background tasks.
    Low = 0,
}

/// Source of an inference request — where it originated.
///
/// Replaces the fragile `request_id < 1000` convention previously used to detect
/// proactive requests. Every `InferenceRequested` event now carries an explicit source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceSource {
    /// User spoke — transcription completed and triggered inference.
    UserVoice,
    /// User typed — text input from CLI or shell.
    UserText,
    /// Proactive CTP trigger — Sena initiated the thought autonomously.
    ProactiveCTP,
    /// Iterative reasoning — a subsequent round in a multi-round inference chain.
    Iterative,
}

/// Inference-layer events.
#[derive(Debug, Clone)]
pub enum InferenceEvent {
    /// A model was discovered during model scanning.
    ModelDiscovered(ModelInfo),
    /// Model registry built successfully after discovery.
    ModelRegistryBuilt {
        model_count: usize,
        default_model: Option<String>,
    },
    /// Model discovery failed — no models available.
    ModelDiscoveryFailed { reason: String },
    /// Inference request submitted.
    InferenceRequested {
        prompt: String,
        priority: Priority,
        request_id: u64,
        /// Where this inference request originated.
        source: InferenceSource,
    },
    /// Multi-round inference request with memory interleave.
    InferenceRequestedIterative {
        prompt: String,
        priority: Priority,
        request_id: u64,
        max_rounds: usize,
    },
    /// Inference response produced.
    InferenceCompleted {
        text: String,
        request_id: u64,
        token_count: usize,
    },
    /// Partial response emitted at the end of each reflection round.
    InferenceRoundCompleted {
        text: String,
        request_id: u64,
        round: usize,
        total_rounds: usize,
    },
    /// Inference request failed.
    InferenceFailed { request_id: u64, reason: String },
    /// A single token produced during streaming inference.
    InferenceTokenGenerated {
        /// The decoded token string.
        token: String,
        /// The request this token belongs to.
        request_id: u64,
        /// Zero-based position of this token in the stream.
        sequence_number: u64,
    },
    /// A complete sentence boundary detected during streaming inference.
    ///
    /// Emitted when `detect_sentence_boundary` finds a hard or soft boundary
    /// in the accumulation buffer. Ready for TTS synthesis.
    InferenceSentenceReady {
        /// The complete sentence text (trimmed, boundary character included).
        sentence: String,
        /// The request this sentence belongs to.
        request_id: u64,
        /// Zero-based index of this sentence in the response stream.
        sentence_index: u64,
    },
    /// Streaming inference for a request has fully completed.
    ///
    /// The full response text is the concatenation of all emitted sentences plus
    /// any trailing content flushed at stream close.
    InferenceStreamCompleted {
        /// Full concatenated response text.
        text: String,
        /// The request this completion belongs to.
        request_id: u64,
        /// Total number of tokens generated.
        total_token_count: u64,
        /// Total number of sentences emitted (including final flush).
        total_sentence_count: u64,
    },
    /// Embedding request submitted.
    EmbedRequested { text: String, request_id: u64 },
    /// Embedding response produced.
    EmbedCompleted { vector: Vec<f32>, request_id: u64 },
    /// Extraction request submitted.
    ExtractionRequested { text: String, request_id: u64 },
    /// Extraction response produced.
    ExtractionCompleted { facts: Vec<String>, request_id: u64 },
    /// Model weights loaded lazily on first request.
    ModelLoaded { name: String, backend: String },
    /// Backend mismatch warning: GPU backend detected but llama-cpp-2 compiled without GPU support.
    BackendMismatchWarning { detected: String, compiled: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn model_discovered_constructs_and_clones() {
        let info = ModelInfo {
            name: "llama2:7b".to_string(),
            path: PathBuf::from("/models/llama2-7b.gguf"),
            size_bytes: 4_000_000_000,
            quantization: Quantization::Q4_0,
        };
        let event = InferenceEvent::ModelDiscovered(info);
        let cloned = event.clone();

        if let InferenceEvent::ModelDiscovered(model_info) = cloned {
            assert_eq!(model_info.name, "llama2:7b");
            assert_eq!(model_info.path, PathBuf::from("/models/llama2-7b.gguf"));
            assert_eq!(model_info.size_bytes, 4_000_000_000);
            assert_eq!(model_info.quantization, Quantization::Q4_0);
        } else {
            panic!("Expected ModelDiscovered variant");
        }
    }

    #[test]
    fn model_registry_built_constructs_and_clones() {
        let event = InferenceEvent::ModelRegistryBuilt {
            model_count: 3,
            default_model: Some("llama2:7b".to_string()),
        };
        let cloned = event.clone();

        if let InferenceEvent::ModelRegistryBuilt {
            model_count,
            default_model,
        } = cloned
        {
            assert_eq!(model_count, 3);
            assert_eq!(default_model, Some("llama2:7b".to_string()));
        } else {
            panic!("Expected ModelRegistryBuilt variant");
        }
    }

    #[test]
    fn model_discovery_failed_constructs_and_clones() {
        let event = InferenceEvent::ModelDiscoveryFailed {
            reason: "no models found".to_string(),
        };
        let cloned = event.clone();

        if let InferenceEvent::ModelDiscoveryFailed { reason } = cloned {
            assert_eq!(reason, "no models found");
        } else {
            panic!("Expected ModelDiscoveryFailed variant");
        }
    }

    #[test]
    fn model_info_clones_independently() {
        let info = ModelInfo {
            name: "test-model".to_string(),
            path: PathBuf::from("/test/path.gguf"),
            size_bytes: 1024,
            quantization: Quantization::F16,
        };
        let cloned = info.clone();
        assert_eq!(cloned.name, "test-model");
        assert_eq!(cloned.path, PathBuf::from("/test/path.gguf"));
        assert_eq!(cloned.size_bytes, 1024);
        assert_eq!(cloned.quantization, Quantization::F16);
    }

    #[test]
    fn quantization_variants_construct_and_compare() {
        assert_eq!(Quantization::Q4_0, Quantization::Q4_0);
        assert_eq!(Quantization::Q4_1, Quantization::Q4_1);
        assert_eq!(Quantization::Q5_0, Quantization::Q5_0);
        assert_eq!(Quantization::Q5_1, Quantization::Q5_1);
        assert_eq!(Quantization::Q8_0, Quantization::Q8_0);
        assert_eq!(Quantization::F16, Quantization::F16);
        assert_eq!(Quantization::F32, Quantization::F32);
        assert_eq!(
            Quantization::Unknown("custom".to_string()),
            Quantization::Unknown("custom".to_string())
        );
        assert_ne!(Quantization::Q4_0, Quantization::Q4_1);
    }

    #[test]
    fn quantization_clones_correctly() {
        let q = Quantization::Unknown("test".to_string());
        let cloned = q.clone();
        assert_eq!(q, cloned);
    }

    // Compile-time verification: all types are Send
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}

    #[test]
    fn types_are_send() {
        assert_send::<InferenceEvent>();
        assert_send::<ModelInfo>();
        assert_send::<Quantization>();
        assert_send::<Priority>();
    }

    #[test]
    fn priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert!(Priority::High > Priority::Low);
    }

    #[test]
    fn inference_requested_constructs_and_clones() {
        let event = InferenceEvent::InferenceRequested {
            prompt: "test prompt".to_string(),
            priority: Priority::High,
            request_id: 42,
            source: InferenceSource::UserText,
        };
        let cloned = event.clone();
        if let InferenceEvent::InferenceRequested {
            prompt,
            priority,
            request_id,
            source,
        } = cloned
        {
            assert_eq!(prompt, "test prompt");
            assert_eq!(priority, Priority::High);
            assert_eq!(request_id, 42);
            assert_eq!(source, InferenceSource::UserText);
        } else {
            panic!("Expected InferenceRequested");
        }
    }

    #[test]
    fn inference_completed_constructs_and_clones() {
        let event = InferenceEvent::InferenceCompleted {
            text: "response text".to_string(),
            request_id: 1,
            token_count: 50,
        };
        let cloned = event.clone();
        if let InferenceEvent::InferenceCompleted {
            text,
            request_id,
            token_count,
        } = cloned
        {
            assert_eq!(text, "response text");
            assert_eq!(request_id, 1);
            assert_eq!(token_count, 50);
        } else {
            panic!("Expected InferenceCompleted");
        }
    }

    #[test]
    fn embed_events_construct_and_clone() {
        let event = InferenceEvent::EmbedCompleted {
            vector: vec![0.1, 0.2, 0.3],
            request_id: 7,
        };
        let cloned = event.clone();
        if let InferenceEvent::EmbedCompleted { vector, request_id } = cloned {
            assert_eq!(vector, vec![0.1, 0.2, 0.3]);
            assert_eq!(request_id, 7);
        } else {
            panic!("Expected EmbedCompleted");
        }
    }

    #[test]
    fn extraction_events_construct_and_clone() {
        let event = InferenceEvent::ExtractionCompleted {
            facts: vec!["fact1".to_string(), "fact2".to_string()],
            request_id: 9,
        };
        let cloned = event.clone();
        if let InferenceEvent::ExtractionCompleted { facts, request_id } = cloned {
            assert_eq!(facts, vec!["fact1", "fact2"]);
            assert_eq!(request_id, 9);
        } else {
            panic!("Expected ExtractionCompleted");
        }
    }

    #[test]
    fn model_loaded_constructs_and_clones() {
        let event = InferenceEvent::ModelLoaded {
            name: "llama2".to_string(),
            backend: "CPU".to_string(),
        };
        let cloned = event.clone();
        if let InferenceEvent::ModelLoaded { name, backend } = cloned {
            assert_eq!(name, "llama2");
            assert_eq!(backend, "CPU");
        } else {
            panic!("Expected ModelLoaded");
        }
    }
}
