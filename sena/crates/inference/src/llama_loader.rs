//! Helper for constructing loaded infer-backed llama adapters.

use crate::backend::InferenceBackend;
use crate::discover_models;
use crate::error::InferenceError;
use crate::stream::InferenceStream;
use crate::types::{
    BackendType, GenerationDiagnostics, InferenceParams, InferenceStopReason,
    PreparedInferenceRequest, QWEN_CHATML_STOP_SEQUENCES,
};
use async_trait::async_trait;
use infer::InferenceBackend as InferBackendTrait;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Mutex, mpsc};
use tracing::info;

const DEFAULT_CTX_SIZE: u32 = 2048;
const QWEN_SYSTEM_PROMPT: &str =
    "You are Sena, a voice-first personal assistant. Answer naturally and directly.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptTemplateProfile {
    Raw,
    QwenChatMl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StopOutcome {
    Emit(String),
    Stop {
        emit: String,
        matched_sequence: String,
    },
    Wait,
}

#[derive(Debug, Clone)]
struct StopSequenceBuffer {
    stop_sequences: Vec<String>,
    pending: String,
}

impl StopSequenceBuffer {
    fn new(stop_sequences: &[String]) -> Self {
        Self {
            stop_sequences: stop_sequences
                .iter()
                .filter(|sequence| !sequence.is_empty())
                .cloned()
                .collect(),
            pending: String::new(),
        }
    }

    fn push(&mut self, piece: &str) -> StopOutcome {
        self.pending.push_str(piece);

        if let Some((stop_index, matched_sequence)) =
            find_earliest_stop(&self.pending, &self.stop_sequences)
        {
            let emit = self.pending[..stop_index].to_string();
            self.pending.clear();
            return StopOutcome::Stop {
                emit,
                matched_sequence,
            };
        }

        let keep_len = longest_partial_stop_suffix(&self.pending, &self.stop_sequences);
        let emit_len = self.pending.len().saturating_sub(keep_len);
        if emit_len == 0 {
            return StopOutcome::Wait;
        }

        let tail = self.pending.split_off(emit_len);
        let emit = std::mem::replace(&mut self.pending, tail);
        StopOutcome::Emit(emit)
    }

    fn finish(self) -> Option<String> {
        if self.pending.is_empty() {
            None
        } else {
            Some(self.pending)
        }
    }
}

fn find_earliest_stop(text: &str, stop_sequences: &[String]) -> Option<(usize, String)> {
    stop_sequences
        .iter()
        .filter_map(|sequence| text.find(sequence).map(|index| (index, sequence.clone())))
        .min_by_key(|(index, _)| *index)
}

fn longest_partial_stop_suffix(text: &str, stop_sequences: &[String]) -> usize {
    let mut longest = 0;

    for sequence in stop_sequences {
        for boundary in sequence
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(sequence.len()))
            .skip(1)
        {
            if boundary >= sequence.len() || boundary <= longest || boundary > text.len() {
                continue;
            }

            if text.ends_with(&sequence[..boundary]) {
                longest = boundary;
            }
        }
    }

    longest
}

fn prompt_template_for_model(model_name: Option<&str>) -> PromptTemplateProfile {
    let Some(model_name) = model_name else {
        return PromptTemplateProfile::Raw;
    };

    let model_name = model_name.to_ascii_lowercase();
    if model_name.contains("qwen") && model_name.contains("instruct") {
        PromptTemplateProfile::QwenChatMl
    } else {
        PromptTemplateProfile::Raw
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn resolve_model_name(model_path: &Path) -> Option<String> {
    let file_stem = model_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string);

    let needs_registry_lookup = file_stem
        .as_deref()
        .map(|stem| stem.starts_with("sha256-"))
        .unwrap_or(true);

    if !needs_registry_lookup {
        return file_stem;
    }

    let models_dir = infer::ollama_models_dir().ok()?;
    let registry = discover_models(&models_dir).ok()?;
    registry
        .models
        .iter()
        .find(|model| paths_match(&model.path, model_path))
        .map(|model| model.name.clone())
        .or(file_stem)
}

pub(crate) fn prepare_inference_request(
    model_name: Option<&str>,
    prompt: &str,
    params: &InferenceParams,
) -> PreparedInferenceRequest {
    let profile = prompt_template_for_model(model_name);
    let mut prepared_params = params.clone();

    match profile {
        PromptTemplateProfile::QwenChatMl if prepared_params.stop_sequences.is_empty() => {
            prepared_params.stop_sequences = QWEN_CHATML_STOP_SEQUENCES
                .iter()
                .map(|sequence| (*sequence).to_string())
                .collect();

            PreparedInferenceRequest {
                prompt: format!(
                    "<|im_start|>system\n{}<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
                    QWEN_SYSTEM_PROMPT,
                    prompt.trim_end()
                ),
                params: prepared_params,
                prompt_template: "qwen_chatml",
            }
        }
        _ => PreparedInferenceRequest {
            prompt: prompt.to_string(),
            params: prepared_params,
            prompt_template: "raw",
        },
    }
}

fn to_infer_params(prompt: String, params: InferenceParams) -> infer::InferenceParams {
    infer::InferenceParams {
        request_id: uuid::Uuid::new_v4(),
        prompt,
        temperature: params.temperature,
        top_k: params.top_k,
        top_p: params.top_p,
        repeat_penalty: params.repeat_penalty,
        max_tokens: params.max_tokens,
        ctx_size: DEFAULT_CTX_SIZE,
        stop_sequences: params.stop_sequences,
        kv_cache: infer::KvCacheConfig::none(),
    }
}

fn to_infer_params_ref(prompt: &str, params: &InferenceParams) -> infer::InferenceParams {
    infer::InferenceParams {
        request_id: uuid::Uuid::new_v4(),
        prompt: prompt.to_string(),
        temperature: params.temperature,
        top_k: params.top_k,
        top_p: params.top_p,
        repeat_penalty: params.repeat_penalty,
        max_tokens: params.max_tokens,
        ctx_size: DEFAULT_CTX_SIZE,
        stop_sequences: params.stop_sequences.clone(),
        kv_cache: infer::KvCacheConfig::none(),
    }
}

struct LlamaBackendAdapter {
    inner: Arc<Mutex<infer::LlamaBackend>>,
    model_name: Option<String>,
    last_generation_diagnostics: Arc<StdMutex<Option<GenerationDiagnostics>>>,
}

impl LlamaBackendAdapter {
    fn new(backend: infer::LlamaBackend, model_name: Option<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(backend)),
            model_name,
            last_generation_diagnostics: Arc::new(StdMutex::new(None)),
        }
    }
}

#[async_trait]
impl InferenceBackend for LlamaBackendAdapter {
    fn backend_type(&self) -> BackendType {
        BackendType::LlamaCpp
    }

    fn is_loaded(&self) -> bool {
        true
    }

    fn model_name(&self) -> Option<&str> {
        self.model_name.as_deref()
    }

    async fn infer(
        &self,
        prompt: String,
        params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError> {
        let stop_sequences = params.stop_sequences.clone();
        let max_tokens = params.max_tokens;
        let infer_params = to_infer_params(
            prompt,
            InferenceParams {
                stop_sequences: Vec::new(),
                ..params
            },
        );
        let backend_clone = Arc::clone(&self.inner);
        let stream_rx = tokio::task::spawn_blocking(move || {
            let backend = backend_clone.blocking_lock();
            backend.stream(infer_params)
        })
        .await
        .map_err(|error| {
            InferenceError::ExecutionFailed(format!("spawn_blocking failed: {}", error))
        })?
        .map_err(|error| InferenceError::ExecutionFailed(format!("stream failed: {}", error)))?;

        let (tx, rx) = mpsc::channel(100);
        let diagnostics_slot = Arc::clone(&self.last_generation_diagnostics);
        tokio::task::spawn_blocking(move || {
            let mut stop_buffer = StopSequenceBuffer::new(&stop_sequences);
            let mut raw_generated_text = String::new();
            let mut generated_token_count = 0usize;
            let mut stop_reason = InferenceStopReason::EosToken;

            *diagnostics_slot
                .lock()
                .expect("llama diagnostics mutex should not be poisoned") = None;

            while let Ok(token) = stream_rx.recv() {
                generated_token_count += 1;
                raw_generated_text.push_str(&token);

                match stop_buffer.push(&token) {
                    StopOutcome::Emit(chunk) => {
                        if !chunk.is_empty() && tx.blocking_send(Ok(chunk)).is_err() {
                            stop_reason = InferenceStopReason::NaturalEnd;
                            break;
                        }
                    }
                    StopOutcome::Stop {
                        emit,
                        matched_sequence,
                    } => {
                        if !emit.is_empty() {
                            let _ = tx.blocking_send(Ok(emit));
                        }
                        stop_reason = InferenceStopReason::StopSequence(matched_sequence);
                        break;
                    }
                    StopOutcome::Wait => {}
                }
            }

            if !matches!(stop_reason, InferenceStopReason::StopSequence(_)) {
                if let Some(tail) = stop_buffer.finish()
                    && !tail.is_empty()
                    && tx.blocking_send(Ok(tail)).is_err()
                {
                    stop_reason = InferenceStopReason::NaturalEnd;
                }

                if generated_token_count >= max_tokens {
                    stop_reason = InferenceStopReason::MaxTokensReached;
                }
            }

            *diagnostics_slot
                .lock()
                .expect("llama diagnostics mutex should not be poisoned") = Some(
                GenerationDiagnostics {
                    generated_token_count,
                    stop_reason,
                    raw_generated_text,
                },
            );
        });

        Ok(InferenceStream::new(rx))
    }

    fn complete(&self, prompt: &str, params: &InferenceParams) -> Result<String, InferenceError> {
        let infer_params = to_infer_params_ref(prompt, params);
        let backend = self
            .inner
            .try_lock()
            .map_err(|_| InferenceError::ExecutionFailed("backend busy".to_string()))?;
        backend
            .complete(&infer_params)
            .map_err(|error| InferenceError::ExecutionFailed(format!("complete failed: {}", error)))
    }

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }

    fn take_generation_diagnostics(&self) -> Option<GenerationDiagnostics> {
        self.last_generation_diagnostics
            .lock()
            .expect("llama diagnostics mutex should not be poisoned")
            .take()
    }
}

pub fn preferred_llama_backend() -> infer::BackendType {
    #[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "vulkan"))]
    {
        infer::BackendType::Vulkan
    }

    #[cfg(all(
        not(all(any(target_os = "windows", target_os = "linux"), feature = "vulkan")),
        target_os = "macos",
        feature = "metal"
    ))]
    {
        infer::BackendType::Metal
    }

    #[cfg(all(
        not(all(any(target_os = "windows", target_os = "linux"), feature = "vulkan")),
        not(all(target_os = "macos", feature = "metal")),
        any(target_os = "windows", target_os = "linux"),
        feature = "cuda"
    ))]
    {
        infer::BackendType::Cuda
    }

    #[cfg(not(any(
        all(any(target_os = "windows", target_os = "linux"), feature = "vulkan"),
        all(target_os = "macos", feature = "metal"),
        all(any(target_os = "windows", target_os = "linux"), feature = "cuda")
    )))]
    {
        infer::BackendType::auto_detect()
    }
}

pub fn build_loaded_llama_backend(
    model_path: &Path,
) -> Result<Box<dyn InferenceBackend>, InferenceError> {
    let backend_type = preferred_llama_backend();
    let model_name = resolve_model_name(model_path);
    let model_size_mb = std::fs::metadata(model_path)
        .map(|metadata| metadata.len() / (1024 * 1024))
        .unwrap_or(0);
    info!(
        compute_backend = %backend_type,
        model_path = %model_path.display(),
        model_size_mb,
        "loading infer llama generation model"
    );

    let mut backend = infer::LlamaBackend::new().map_err(|error| {
        InferenceError::BackendInit(format!("llama backend init failed: {}", error))
    })?;
    backend
        .load_model(model_path, backend_type)
        .map_err(|error| {
            InferenceError::BackendFailed(format!(
                "failed to load model from {}: {}",
                model_path.display(),
                error
            ))
        })?;

    info!(
        compute_backend = %backend_type,
        model_path = %model_path.display(),
        model_size_mb,
        prompt_template = %match prompt_template_for_model(model_name.as_deref()) {
            PromptTemplateProfile::QwenChatMl => "qwen_chatml",
            PromptTemplateProfile::Raw => "raw",
        },
        "infer llama generation model loaded"
    );

    Ok(Box::new(LlamaBackendAdapter::new(backend, model_name)))
}

struct LlamaEmbedBackendAdapter {
    inner: Arc<Mutex<infer::LlamaEmbedBackend>>,
}

impl LlamaEmbedBackendAdapter {
    fn new(backend: infer::LlamaEmbedBackend) -> Self {
        Self {
            inner: Arc::new(Mutex::new(backend)),
        }
    }
}

#[async_trait]
impl InferenceBackend for LlamaEmbedBackendAdapter {
    fn backend_type(&self) -> BackendType {
        BackendType::LlamaCpp
    }

    fn is_loaded(&self) -> bool {
        true
    }

    async fn infer(
        &self,
        _prompt: String,
        _params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError> {
        Err(InferenceError::ExecutionFailed(
            "LlamaEmbedBackend is an embedding-only backend".to_string(),
        ))
    }

    async fn embed(&self, text: String) -> Result<Vec<f32>, InferenceError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let backend = inner.blocking_lock();
            backend.embed(&text).map_err(|error| {
                InferenceError::ExecutionFailed(format!("embed failed: {}", error))
            })
        })
        .await
        .map_err(|error| {
            InferenceError::ExecutionFailed(format!("embed: spawn_blocking: {error}"))
        })?
    }

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }
}

pub fn build_loaded_embed_backend(
    model_path: &Path,
) -> Result<Box<dyn InferenceBackend>, InferenceError> {
    let backend_type = preferred_llama_backend();
    let backend = infer::LlamaEmbedBackend::load(model_path, backend_type).map_err(|error| {
        InferenceError::BackendFailed(format!(
            "embed: failed to load model {}: {}",
            model_path.display(),
            error
        ))
    })?;

    Ok(Box::new(LlamaEmbedBackendAdapter::new(backend)))
}

#[cfg(test)]
mod tests {
    use super::{prepare_inference_request, prompt_template_for_model, resolve_model_name, QWEN_SYSTEM_PROMPT};
    use crate::types::{InferenceParams, InferenceStopReason, QWEN_CHATML_STOP_SEQUENCES};
    use std::path::Path;

    #[test]
    fn preferred_backend_type_is_selectable() {
        let backend = super::preferred_llama_backend();
        assert!(!backend.to_string().is_empty());
    }

    #[test]
    fn qwen_models_are_wrapped_in_chatml_when_stops_are_backend_derived() {
        let prepared = prepare_inference_request(
            Some("Qwen2.5-7B-Instruct-Q4_K_M"),
            "hello there",
            &InferenceParams::default(),
        );

        assert_eq!(prepared.prompt_template, "qwen_chatml");
        assert!(prepared.prompt.starts_with("<|im_start|>system\n"));
        assert!(prepared.prompt.contains(QWEN_SYSTEM_PROMPT));
        assert!(prepared.prompt.ends_with("<|im_start|>assistant\n"));
        assert_eq!(
            prepared.params.stop_sequences,
            QWEN_CHATML_STOP_SEQUENCES
                .iter()
                .map(|sequence| (*sequence).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn explicit_stop_sequences_keep_raw_prompt_shape() {
        let mut params = InferenceParams::default();
        params.stop_sequences = vec!["\n\n".to_string()];

        let prepared = prepare_inference_request(
            Some("Qwen2.5-7B-Instruct-Q4_K_M"),
            "structured extraction prompt",
            &params,
        );

        assert_eq!(prepared.prompt_template, "raw");
        assert_eq!(prepared.prompt, "structured extraction prompt");
        assert_eq!(prepared.params.stop_sequences, vec!["\n\n".to_string()]);
    }

    #[test]
    fn stop_reason_formatting_is_exact_for_qwen_stop_token() {
        assert_eq!(
            InferenceStopReason::StopSequence("<|im_end|>".to_string()).as_log_value(),
            "stop_sequence: <|im_end|>"
        );
    }

    #[test]
    fn qwen_alias_selects_qwen_prompt_template() {
        assert_eq!(
            prompt_template_for_model(Some("qwen2.5:7b-instruct")),
            super::PromptTemplateProfile::QwenChatMl
        );
    }

    #[test]
    fn non_blob_paths_keep_file_stem_as_model_name() {
        assert_eq!(
            resolve_model_name(Path::new("C:\\models\\qwen2.5-7b-instruct.gguf")),
            Some("qwen2.5-7b-instruct".to_string())
        );
    }
}
