//! Production LLM backend using llama-cpp-2.
//!
//! Wraps the llama-cpp-2 crate to provide GGUF model loading, text
//! generation, embedding extraction, and structured fact extraction.
//! All methods are synchronous — callers must use `spawn_blocking`.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend as LlamaCppInit;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::TokenToStringError;

use crate::backend::{BackendError, BackendType, InferenceParams, LlmBackend};

/// Global one-time llama.cpp backend initialisation.
/// Stores a `Result` so that init failure is remembered and not retried.
static LLAMA_CPP_INIT: OnceLock<Result<LlamaCppInit, String>> = OnceLock::new();

fn init_llama_cpp() -> Result<&'static LlamaCppInit, BackendError> {
    let result = LLAMA_CPP_INIT
        .get_or_init(|| LlamaCppInit::init().map_err(|e| format!("llama backend init: {e}")));
    result
        .as_ref()
        .map_err(|e| BackendError::ModelLoadFailed(e.clone()))
}

/// Default context size cap (tokens).
const DEFAULT_CTX_SIZE: u32 = 4096;

/// Production LLM backend backed by llama-cpp-2.
pub struct LlamaBackend {
    model: Option<LlamaModel>,
}

impl LlamaBackend {
    /// Create a new `LlamaBackend` (model not yet loaded).
    pub fn new() -> Self {
        Self { model: None }
    }
}

impl Default for LlamaBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a slice of [`LlamaToken`] back into a UTF-8 string using
/// the raw byte API so we do not need the `encoding_rs` dependency.
fn detokenize(model: &LlamaModel, tokens: &[LlamaToken]) -> Result<String, BackendError> {
    let mut bytes: Vec<u8> = Vec::with_capacity(tokens.len() * 4);
    for token in tokens {
        let piece = match model.token_to_piece_bytes(*token, 32, false, None) {
            Ok(b) => b,
            Err(TokenToStringError::InsufficientBufferSpace(needed)) => {
                let size = usize::try_from(-needed).unwrap_or(256);
                model
                    .token_to_piece_bytes(*token, size, false, None)
                    .map_err(|e| BackendError::InferenceFailed(format!("detokenize: {e}")))?
            }
            Err(e) => {
                return Err(BackendError::InferenceFailed(format!("detokenize: {e}")));
            }
        };
        bytes.extend_from_slice(&piece);
    }
    String::from_utf8(bytes).map_err(|e| BackendError::InferenceFailed(format!("utf8 decode: {e}")))
}

impl LlmBackend for LlamaBackend {
    fn load_model(
        &mut self,
        model_path: &Path,
        backend_type: BackendType,
    ) -> Result<(), BackendError> {
        if !model_path.exists() {
            return Err(BackendError::ModelLoadFailed(format!(
                "model file not found: {}",
                model_path.display()
            )));
        }

        let backend = init_llama_cpp()?;

        let n_gpu_layers: u32 = match backend_type {
            BackendType::Metal | BackendType::Cuda => 999,
            BackendType::Cpu => 0,
        };

        let model_params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);

        let model = LlamaModel::load_from_file(backend, model_path, &model_params)
            .map_err(|e| BackendError::ModelLoadFailed(format!("{e}")))?;

        self.model = Some(model);
        Ok(())
    }

    fn infer(&self, prompt: &str, params: &InferenceParams) -> Result<String, BackendError> {
        let model = self.model.as_ref().ok_or(BackendError::NotInitialized)?;
        let backend = init_llama_cpp()?;

        // Tokenize prompt
        let tokens = model
            .str_to_token(prompt, AddBos::Always)
            .map_err(|e| BackendError::InferenceFailed(format!("tokenization: {e}")))?;

        if tokens.is_empty() {
            return Ok(String::new());
        }

        // Context size: min of model training context and our cap, but at least prompt length
        let model_ctx = model.n_ctx_train();
        let ctx_size = std::cmp::min(model_ctx, DEFAULT_CTX_SIZE);
        let ctx_size = std::cmp::max(ctx_size, tokens.len() as u32 + params.max_tokens as u32);
        let n_ctx = NonZeroU32::new(ctx_size)
            .ok_or_else(|| BackendError::InferenceFailed("context size is zero".into()))?;

        let ctx_params = LlamaContextParams::default().with_n_ctx(Some(n_ctx));
        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| BackendError::InferenceFailed(format!("context creation: {e}")))?;

        // Evaluate the prompt tokens
        let mut batch = LlamaBatch::new(tokens.len(), 1);
        batch
            .add_sequence(&tokens, 0, false)
            .map_err(|e| BackendError::InferenceFailed(format!("batch: {e}")))?;

        ctx.decode(&mut batch)
            .map_err(|e| BackendError::InferenceFailed(format!("prompt decode: {e}")))?;

        // Build sampler chain: temperature → top-p → dist
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::temp(params.temperature),
            LlamaSampler::top_p(params.top_p, 1),
            LlamaSampler::dist(42),
        ]);

        // Autoregressive generation loop
        let mut output_tokens: Vec<LlamaToken> = Vec::with_capacity(params.max_tokens);
        let mut n_decoded = tokens.len();

        for _ in 0..params.max_tokens {
            let new_token = sampler.sample(&ctx, -1);
            sampler.accept(new_token);

            if model.is_eog_token(new_token) {
                break;
            }

            output_tokens.push(new_token);

            // Prepare next decode step
            batch.clear();
            let pos = i32::try_from(n_decoded)
                .map_err(|_| BackendError::InferenceFailed("position overflow".into()))?;
            batch
                .add(new_token, pos, &[0], true)
                .map_err(|e| BackendError::InferenceFailed(format!("batch add: {e}")))?;

            ctx.decode(&mut batch)
                .map_err(|e| BackendError::InferenceFailed(format!("decode step: {e}")))?;

            n_decoded += 1;
        }

        detokenize(model, &output_tokens)
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, BackendError> {
        let model = self.model.as_ref().ok_or(BackendError::NotInitialized)?;
        let backend = init_llama_cpp()?;

        let tokens = model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| BackendError::EmbeddingFailed(format!("tokenization: {e}")))?;

        if tokens.is_empty() {
            return Err(BackendError::EmbeddingFailed("empty input".into()));
        }

        let n_ctx = NonZeroU32::new(std::cmp::max(tokens.len() as u32, 512))
            .ok_or_else(|| BackendError::EmbeddingFailed("context size is zero".into()))?;

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(n_ctx))
            .with_embeddings(true);

        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| BackendError::EmbeddingFailed(format!("context creation: {e}")))?;

        let mut batch = LlamaBatch::new(tokens.len(), 1);
        batch
            .add_sequence(&tokens, 0, true)
            .map_err(|e| BackendError::EmbeddingFailed(format!("batch: {e}")))?;

        // Embedding models use encode; generative models fall back to decode
        if ctx.encode(&mut batch).is_err() {
            ctx.decode(&mut batch)
                .map_err(|e| BackendError::EmbeddingFailed(format!("decode: {e}")))?;
        }

        // Try sequence-level (pooled) embeddings first, then token-level
        let embeddings = ctx
            .embeddings_seq_ith(0)
            .or_else(|_| {
                let last = i32::try_from(tokens.len().saturating_sub(1)).unwrap_or(0);
                ctx.embeddings_ith(last)
            })
            .map_err(|e| BackendError::EmbeddingFailed(format!("{e}")))?;

        Ok(embeddings.to_vec())
    }

    fn extract(&self, text: &str) -> Result<Vec<String>, BackendError> {
        // `text` is the fully-composed extraction prompt (composed by the caller).
        // We run inference with low temperature for deterministic output and
        // split the result into non-empty lines.
        let params = InferenceParams {
            temperature: 0.1,
            top_p: 0.9,
            max_tokens: 512,
        };

        let result = self.infer(text, &params)?;
        let facts: Vec<String> = result
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();

        Ok(facts)
    }

    fn is_loaded(&self) -> bool {
        self.model.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_backend_starts_unloaded() {
        let backend = LlamaBackend::new();
        assert!(!backend.is_loaded());
    }

    #[test]
    fn llama_backend_default_is_unloaded() {
        let backend = LlamaBackend::default();
        assert!(!backend.is_loaded());
    }

    #[test]
    fn infer_fails_before_load() {
        let backend = LlamaBackend::new();
        let err = backend
            .infer("test", &InferenceParams::default())
            .unwrap_err();
        assert!(
            matches!(err, BackendError::NotInitialized),
            "expected NotInitialized, got {err}"
        );
    }

    #[test]
    fn embed_fails_before_load() {
        let backend = LlamaBackend::new();
        let err = backend.embed("test").unwrap_err();
        assert!(
            matches!(err, BackendError::NotInitialized),
            "expected NotInitialized, got {err}"
        );
    }

    #[test]
    fn extract_fails_before_load() {
        let backend = LlamaBackend::new();
        let err = backend.extract("test").unwrap_err();
        assert!(
            matches!(err, BackendError::NotInitialized),
            "expected NotInitialized, got {err}"
        );
    }

    #[test]
    fn load_model_fails_on_nonexistent_path() {
        let mut backend = LlamaBackend::new();
        let result = backend.load_model(Path::new("/nonexistent/model.gguf"), BackendType::Cpu);
        let err = result.expect_err("expected error for nonexistent model path");
        assert!(
            matches!(err, BackendError::ModelLoadFailed(_)),
            "expected ModelLoadFailed, got {err}"
        );
        assert!(!backend.is_loaded());
    }

    #[test]
    fn load_model_fails_on_invalid_gguf() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fake_model = dir.path().join("fake.gguf");
        std::fs::write(&fake_model, b"not a valid gguf").expect("write fake model");

        let mut backend = LlamaBackend::new();
        let result = backend.load_model(&fake_model, BackendType::Cpu);
        assert!(result.is_err(), "expected error for invalid GGUF file");
        assert!(!backend.is_loaded());
    }
}
