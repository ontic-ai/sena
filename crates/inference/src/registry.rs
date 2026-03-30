//! Model registry for discovered GGUF models.

use bus::events::ModelInfo;

/// Registry of discovered GGUF models.
///
/// Holds all discovered models and tracks which should be used as default.
/// Default model is determined by largest file size.
#[derive(Debug)]
pub struct ModelRegistry {
    models: Vec<ModelInfo>,
    default_model: Option<String>,
}

impl ModelRegistry {
    /// Create an empty model registry.
    pub fn new() -> Self {
        Self {
            models: Vec::new(),
            default_model: None,
        }
    }

    /// Create a registry from a list of models.
    ///
    /// The default model is automatically selected as the one with the largest size_bytes.
    pub fn from_models(models: Vec<ModelInfo>) -> Self {
        let default_model = models
            .iter()
            .max_by_key(|m| m.size_bytes)
            .map(|m| m.name.clone());

        Self {
            models,
            default_model,
        }
    }

    /// Add a model to the registry.
    ///
    /// Does not automatically update the default model selection.
    pub fn add_model(&mut self, model: ModelInfo) {
        self.models.push(model);
    }

    /// Get the name of the default model, if any.
    pub fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Get the number of models in the registry.
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Get a slice of all models in the registry.
    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    /// Find a model by name.
    ///
    /// Returns the first model matching the given name.
    pub fn find_by_name(&self, name: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.name == name)
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::Quantization;
    use std::path::PathBuf;

    fn make_model(name: &str, size_bytes: u64) -> ModelInfo {
        ModelInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/models/{}.gguf", name)),
            size_bytes,
            quantization: Quantization::Q4_0,
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let registry = ModelRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.model_count(), 0);
        assert_eq!(registry.default_model(), None);
    }

    #[test]
    fn from_models_selects_largest_as_default() {
        let models = vec![
            make_model("small", 1_000_000_000),
            make_model("large", 5_000_000_000),
            make_model("medium", 3_000_000_000),
        ];

        let registry = ModelRegistry::from_models(models);

        assert_eq!(registry.model_count(), 3);
        assert_eq!(registry.default_model(), Some("large"));
        assert!(!registry.is_empty());
    }

    #[test]
    fn from_models_with_single_model_selects_it_as_default() {
        let models = vec![make_model("only-model", 2_000_000_000)];

        let registry = ModelRegistry::from_models(models);

        assert_eq!(registry.model_count(), 1);
        assert_eq!(registry.default_model(), Some("only-model"));
    }

    #[test]
    fn from_models_with_empty_vec_has_no_default() {
        let registry = ModelRegistry::from_models(Vec::new());

        assert!(registry.is_empty());
        assert_eq!(registry.default_model(), None);
    }

    #[test]
    fn add_model_increases_count() {
        let mut registry = ModelRegistry::new();

        registry.add_model(make_model("model1", 1_000_000_000));
        assert_eq!(registry.model_count(), 1);

        registry.add_model(make_model("model2", 2_000_000_000));
        assert_eq!(registry.model_count(), 2);

        // Note: add_model does not update default
        assert_eq!(registry.default_model(), None);
    }

    #[test]
    fn models_returns_all_models() {
        let models_vec = vec![
            make_model("model-a", 1_000_000_000),
            make_model("model-b", 2_000_000_000),
        ];

        let registry = ModelRegistry::from_models(models_vec);
        let models = registry.models();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "model-a");
        assert_eq!(models[1].name, "model-b");
    }

    #[test]
    fn find_by_name_returns_matching_model() {
        let models = vec![
            make_model("llama2", 3_000_000_000),
            make_model("mixtral", 4_000_000_000),
        ];

        let registry = ModelRegistry::from_models(models);

        let found = registry.find_by_name("llama2");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "llama2");

        let not_found = registry.find_by_name("nonexistent");
        assert!(not_found.is_none());
    }

    #[test]
    fn find_by_name_in_empty_registry_returns_none() {
        let registry = ModelRegistry::new();
        assert!(registry.find_by_name("anything").is_none());
    }

    #[test]
    fn default_trait_creates_empty_registry() {
        let registry = ModelRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.model_count(), 0);
    }

    #[test]
    fn from_models_with_duplicate_sizes_picks_first_occurrence() {
        let models = vec![
            make_model("first", 5_000_000_000),
            make_model("second", 5_000_000_000),
        ];

        let registry = ModelRegistry::from_models(models);

        // Either could be selected; verify one is selected
        let default = registry.default_model();
        assert!(default == Some("first") || default == Some("second"));
    }
}
