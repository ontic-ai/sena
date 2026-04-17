//! Inference parameter types and backend type enumeration.

use serde::{Deserialize, Serialize};

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
            stop_sequences: Vec::new(),
            repeat_penalty: 1.1,
        }
    }
}

use std::fmt;
