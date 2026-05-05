//! Model management events.

use serde::{Deserialize, Serialize};

/// Detailed information about a discovered model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    /// Friendly name of the model.
    pub name: String,
    /// Absolute path to the model file.
    pub path: String,
    /// Size of the model file in bytes.
    pub size_bytes: u64,
}

/// Model management events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelEvent {
    /// A model was discovered during scanning.
    ModelDiscovered { model_name: String, path: String },

    /// Model registry built successfully.
    RegistryBuilt {
        model_count: usize,
        default_model: Option<String>,
    },

    /// Model discovery failed.
    DiscoveryFailed { reason: String },

    /// Request to switch to a different model.
    SwitchRequested { model_name: String },

    /// Model switch completed successfully.
    SwitchCompleted { model_name: String },

    /// Model switch failed.
    SwitchFailed { model_name: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_events_are_cloneable() {
        let event = ModelEvent::ModelDiscovered {
            model_name: "test-model".to_string(),
            path: "/path/to/model".to_string(),
        };
        let cloned = event.clone();
        assert!(matches!(cloned, ModelEvent::ModelDiscovered { .. }));
    }
}
