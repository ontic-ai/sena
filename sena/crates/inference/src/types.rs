//! Inference parameter types and backend type enumeration.

use serde::{Deserialize, Serialize};

pub const DEFAULT_STOP_SEQUENCES: [&str; 3] = ["\nUser:", "\nAssistant:", "\nSena:"];

/// Live-tunable conversation settings for user-facing inference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationConfig {
    /// Maximum number of tokens to generate per response.
    pub max_tokens: u32,
    /// Temperature for sampling.
    pub temperature: f32,
    /// Repetition penalty.
    pub repeat_penalty: f32,
    /// Top-k sampling.
    pub top_k: u32,
    /// Top-p (nucleus sampling).
    pub top_p: f32,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            max_tokens: 150,
            temperature: 0.7,
            repeat_penalty: 1.15,
            top_k: 40,
            top_p: 0.9,
        }
    }
}

/// Backend type for inference execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BackendType {
    /// llama.cpp backend (future: via llama-cpp-rs).
    LlamaCpp,
    /// Mock backend for testing.
    Mock,
}

impl fmt::Display for BackendType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LlamaCpp => write!(f, "llama.cpp"),
            Self::Mock => write!(f, "mock"),
        }
    }
}

/// Inference parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceParams {
    /// Temperature for sampling.
    pub temperature: f32,
    /// Top-p (nucleus sampling).
    pub top_p: f32,
    /// Top-k sampling.
    pub top_k: u32,
    /// Maximum number of tokens to generate.
    pub max_tokens: usize,
    /// Stop sequences.
    pub stop_sequences: Vec<String>,
    /// Repetition penalty.
    pub repeat_penalty: f32,
}

impl Default for InferenceParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            max_tokens: 512,
            stop_sequences: DEFAULT_STOP_SEQUENCES
                .iter()
                .map(|sequence| (*sequence).to_string())
                .collect(),
            repeat_penalty: 1.1,
        }
    }
}

impl ConversationConfig {
    pub fn to_inference_params(&self) -> InferenceParams {
        InferenceParams {
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            max_tokens: self.max_tokens as usize,
            stop_sequences: DEFAULT_STOP_SEQUENCES
                .iter()
                .map(|sequence| (*sequence).to_string())
                .collect(),
            repeat_penalty: self.repeat_penalty,
        }
    }
}

use std::fmt;

#[cfg(test)]
mod tests {
    use super::{ConversationConfig, DEFAULT_STOP_SEQUENCES, InferenceParams};

    #[test]
    fn default_stop_sequences_match_dialogue_contract() {
        let params = InferenceParams::default();
        let expected: Vec<String> = DEFAULT_STOP_SEQUENCES
            .iter()
            .map(|sequence| (*sequence).to_string())
            .collect();
        assert_eq!(params.stop_sequences, expected);
    }

    #[test]
    fn conversation_config_maps_to_inference_params() {
        let config = ConversationConfig::default();
        let params = config.to_inference_params();

        assert_eq!(params.max_tokens, 150);
        assert_eq!(params.temperature, 0.7);
        assert_eq!(params.repeat_penalty, 1.15);
        assert_eq!(params.top_k, 40);
        assert_eq!(params.top_p, 0.9);
    }
}
