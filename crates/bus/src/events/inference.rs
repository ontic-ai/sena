//! Inference-layer events: model discovery, requests, responses.

use std::path::PathBuf;

/// Quantization level of a GGUF model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quantization {
    Q4_0,
    Q4_1,
    Q5_0,
    Q5_1,
    Q8_0,
    F16,
    F32,
    Unknown(String),
}

/// Information about a discovered GGUF model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Human-readable model name (e.g., "llama2:7b").
    pub name: String,
    /// Absolute path to the GGUF file.
    pub path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Quantization level parsed from model metadata or filename.
    pub quantization: Quantization,
}

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let cloned = event.clone();
        if let InferenceEvent::InferenceRequested {
            prompt,
            priority,
            request_id,
        } = cloned
        {
            assert_eq!(prompt, "test prompt");
            assert_eq!(priority, Priority::High);
            assert_eq!(request_id, 42);
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
