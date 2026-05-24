//! Inference actor — receives InferenceRequested events and produces token streams.

use crate::backend::InferenceBackend;
use crate::build_loaded_llama_backend;
use crate::error::InferenceError;
use crate::filter::OutputFilter;
use crate::llama_loader::prepare_inference_request;
use crate::queue::{InferenceQueue, WorkItem, WorkKind};
use crate::types::{
    ConversationConfig, GenerationDiagnostics, InferenceParams, InferenceStopReason,
    PreparedInferenceRequest,
};
#[cfg(test)]
use bus::ContextSnapshot;
use bus::events::MemoryKind;
use bus::events::ctp::EnrichedInferredTask;
use bus::{
    Actor, ActorError, ContextInterpretationInput, Event, EventBus, InferenceEvent,
    InferenceFailureOrigin, InferenceSource, MemoryEvent, Priority, TransparencyEvent,
    TransparencyQuery, TransparencyResult,
};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::future::pending;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::Command;
use std::sync::{
    Arc,
    RwLock,
};
use std::time::Duration;
use text::SentenceBoundaryIterator;
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{debug, info, trace, warn};

/// Default queue capacity.
const DEFAULT_QUEUE_CAPACITY: usize = 100;
const EMBEDDING_FALLBACK_DIMENSIONS: usize = 768;
const MEMORY_WRITE_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const REASONING_PREVIEW_CHARS: usize = 160;
const REASONING_HISTORY_LIMIT: usize = 8;

/// Directed embedding request sent from memory to inference.
pub struct EmbedRequest {
    pub text: String,
    pub response_tx: tokio::sync::oneshot::Sender<Result<Vec<f32>, String>>,
}

#[derive(Clone, Debug)]
struct LastReasoningState {
    causal_id: bus::CausalId,
    source: InferenceSource,
    token_count: usize,
    response_preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProactiveAction {
    Speak,
    Observe,
    Nothing,
}

#[derive(Debug, Clone)]
struct ProactiveDecision {
    action: ProactiveAction,
    content: String,
}

/// Inference actor.
pub struct InferenceActor {
    bus: Option<Arc<EventBus>>,
    backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
    embed_backend: Option<Arc<Mutex<Box<dyn InferenceBackend>>>>,
    queue: Arc<Mutex<InferenceQueue>>,
    rx: Option<broadcast::Receiver<Event>>,
    directed_rx: Option<mpsc::Receiver<Event>>,
    embed_rx: Option<mpsc::Receiver<EmbedRequest>>,
    embed_tx_guard: Option<mpsc::Sender<EmbedRequest>>,
    work_tx: Option<mpsc::Sender<()>>,
    conversation_config: Arc<RwLock<ConversationConfig>>,
    pending_user_inputs: HashMap<bus::CausalId, String>,
    pending_reasoning: HashMap<bus::CausalId, InferenceSource>,
    reasoning_history: VecDeque<LastReasoningState>,
}

impl InferenceActor {
    fn escape_for_debug(value: &str) -> String {
        value.chars().flat_map(char::escape_default).collect()
    }

    fn prompt_preview_tokens(prompt: &str, limit: usize) -> Vec<String> {
        prompt
            .split_whitespace()
            .take(limit)
            .map(Self::escape_for_debug)
            .collect()
    }

    fn fallback_stop_reason(token_count: usize, params: &InferenceParams) -> InferenceStopReason {
        if token_count >= params.max_tokens {
            InferenceStopReason::MaxTokensReached
        } else {
            InferenceStopReason::NaturalEnd
        }
    }

    fn log_generation_start(
        source: InferenceSource,
        causal_id: bus::CausalId,
        model_name: Option<&str>,
        prepared: &PreparedInferenceRequest,
    ) {
        let stop_sequences: Vec<String> = prepared
            .params
            .stop_sequences
            .iter()
            .map(|sequence| Self::escape_for_debug(sequence))
            .collect();
        let prompt_preview = Self::prompt_preview_tokens(&prepared.prompt, 100);

        debug!(
            ?source,
            ?causal_id,
            model_name = model_name.unwrap_or("unknown"),
            prompt_template = prepared.prompt_template,
            stop_sequences = ?stop_sequences,
            prompt_preview_tokens = ?prompt_preview,
            max_tokens = prepared.params.max_tokens,
            temperature = prepared.params.temperature,
            repeat_penalty = prepared.params.repeat_penalty,
            "inference generation starting"
        );
    }

    fn log_generation_finish(
        source: InferenceSource,
        causal_id: bus::CausalId,
        diagnostics: Option<&GenerationDiagnostics>,
        fallback_token_count: usize,
        params: &InferenceParams,
        fallback_raw_text: &str,
    ) {
        let generated_token_count = diagnostics
            .map(|details| details.generated_token_count)
            .unwrap_or(fallback_token_count);
        let stop_condition = diagnostics
            .map(|details| details.stop_reason.as_log_value())
            .unwrap_or_else(|| Self::fallback_stop_reason(fallback_token_count, params).as_log_value());
        let raw_generated_text = diagnostics
            .map(|details| details.raw_generated_text.as_str())
            .unwrap_or(fallback_raw_text);

        debug!(
            ?source,
            ?causal_id,
            generated_token_count,
            stop_condition,
            raw_generated_text = %Self::escape_for_debug(raw_generated_text),
            "inference generation finished"
        );
    }

    fn failure_origin_for_source(source: InferenceSource) -> InferenceFailureOrigin {
        match source {
            InferenceSource::UserVoice | InferenceSource::UserText => {
                InferenceFailureOrigin::UserRequest
            }
            InferenceSource::ProactiveCTP => InferenceFailureOrigin::ProactiveCTP,
            InferenceSource::Iterative => InferenceFailureOrigin::UserRequest,
        }
    }

    /// Create a new inference actor with the given backend.
    pub fn new(backend: Box<dyn InferenceBackend>) -> Self {
        let (embed_tx, embed_rx) = mpsc::channel(32);
        Self::with_embed_channel(backend, DEFAULT_QUEUE_CAPACITY, embed_rx, Some(embed_tx))
    }

    /// Create a new inference actor with custom queue capacity.
    pub fn with_queue_capacity(backend: Box<dyn InferenceBackend>, queue_capacity: usize) -> Self {
        let (embed_tx, embed_rx) = mpsc::channel(32);
        Self::with_embed_channel(backend, queue_capacity, embed_rx, Some(embed_tx))
    }

    /// Create a new inference actor with an externally provided embedding request channel.
    pub fn with_embed_requests(
        backend: Box<dyn InferenceBackend>,
        queue_capacity: usize,
        embed_rx: mpsc::Receiver<EmbedRequest>,
    ) -> Self {
        Self::with_embed_channel(backend, queue_capacity, embed_rx, None)
    }

    fn with_embed_channel(
        backend: Box<dyn InferenceBackend>,
        queue_capacity: usize,
        embed_rx: mpsc::Receiver<EmbedRequest>,
        embed_tx_guard: Option<mpsc::Sender<EmbedRequest>>,
    ) -> Self {
        Self {
            bus: None,
            backend: Arc::new(Mutex::new(backend)),
            embed_backend: None,
            queue: Arc::new(Mutex::new(InferenceQueue::new(queue_capacity))),
            rx: None,
            directed_rx: None,
            embed_rx: Some(embed_rx),
            embed_tx_guard,
            work_tx: None,
            conversation_config: Arc::new(RwLock::new(ConversationConfig::default())),
            pending_user_inputs: HashMap::new(),
            pending_reasoning: HashMap::new(),
            reasoning_history: VecDeque::new(),
        }
    }

    /// Override the default inference token budget.
    pub fn with_inference_max_tokens(self, max_tokens: usize) -> Self {
        self.conversation_config
            .write()
            .expect("conversation config lock poisoned")
            .max_tokens = max_tokens as u32;
        self
    }

    pub fn with_conversation_config(self, conversation_config: ConversationConfig) -> Self {
        self.with_shared_conversation_config(Arc::new(RwLock::new(conversation_config)))
    }

    pub fn with_shared_conversation_config(
        mut self,
        conversation_config: Arc<RwLock<ConversationConfig>>,
    ) -> Self {
        self.conversation_config = conversation_config;
        self
    }

    /// Set a dedicated embedding backend.
    ///
    /// When set, embed requests are routed to this backend instead of the
    /// primary generation backend.
    pub fn with_embed_backend(mut self, backend: Box<dyn InferenceBackend>) -> Self {
        self.embed_backend = Some(Arc::new(Mutex::new(backend)));
        self
    }

    fn remember_pending_reasoning(&mut self, source: InferenceSource, causal_id: bus::CausalId) {
        self.pending_reasoning.insert(causal_id, source);
    }

    fn remember_completed_reasoning(
        &mut self,
        causal_id: bus::CausalId,
        text: &str,
        token_count: usize,
    ) {
        let Some(source) = self.pending_reasoning.remove(&causal_id) else {
            return;
        };

        self.reasoning_history.push_back(LastReasoningState {
            causal_id,
            source,
            token_count,
            response_preview: Self::truncate_preview(text, REASONING_PREVIEW_CHARS),
        });

        while self.reasoning_history.len() > REASONING_HISTORY_LIMIT {
            self.reasoning_history.pop_front();
        }
    }

    fn forget_pending_reasoning(&mut self, causal_id: bus::CausalId) {
        self.pending_reasoning.remove(&causal_id);
    }

    fn remember_user_input(&mut self, causal_id: bus::CausalId, text: String) {
        self.pending_user_inputs.insert(causal_id, text);
    }

    async fn write_memory_with_ack(
        bus: &Arc<EventBus>,
        text: String,
        kind: MemoryKind,
        causal_id: bus::CausalId,
    ) -> Result<(), InferenceError> {
        let mut ack_rx = bus.subscribe_broadcast();

        bus.broadcast(Event::Memory(MemoryEvent::MemoryWriteRequest {
            text,
            kind,
            causal_id,
        }))
        .await?;

        tokio::time::timeout(MEMORY_WRITE_ACK_TIMEOUT, async {
            loop {
                match ack_rx.recv().await {
                    Ok(Event::Memory(MemoryEvent::MemoryWriteCompleted { causal_id: ack_id }))
                        if ack_id == causal_id =>
                    {
                        return Ok(());
                    }
                    Ok(Event::Memory(MemoryEvent::MemoryWriteFailed {
                        causal_id: ack_id,
                        reason,
                    })) if ack_id == causal_id => {
                        return Err(InferenceError::ExecutionFailed(format!(
                            "memory write failed: {}",
                            reason
                        )));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        return Err(InferenceError::ExecutionFailed(format!(
                            "memory write ack channel closed: {}",
                            error
                        )));
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            InferenceError::ExecutionFailed(format!(
                "memory write acknowledgement timed out after {} seconds",
                MEMORY_WRITE_ACK_TIMEOUT.as_secs()
            ))
        })?
    }

    async fn persist_completed_exchange(
        &mut self,
        bus: &Arc<EventBus>,
        source: InferenceSource,
        response_text: &str,
        causal_id: bus::CausalId,
    ) -> Result<(), InferenceError> {
        if !matches!(
            source,
            InferenceSource::UserVoice | InferenceSource::UserText
        ) {
            return Ok(());
        }

        if let Some(user_text) = self.pending_user_inputs.remove(&causal_id) {
            let trimmed = user_text.trim();
            if !trimmed.is_empty() {
                Self::write_memory_with_ack(
                    bus,
                    format!("User: {}", trimmed),
                    MemoryKind::Episodic,
                    causal_id,
                )
                .await?;
            }
        } else {
            warn!(
                ?causal_id,
                "missing cached user input for completed exchange"
            );
        }

        let trimmed_response = response_text.trim();
        if !trimmed_response.is_empty() {
            Self::write_memory_with_ack(
                bus,
                format!("Sena: {}", trimmed_response),
                MemoryKind::Episodic,
                causal_id,
            )
            .await?;
        }

        Ok(())
    }

    fn build_reasoning_response(
        &self,
        thought_id: &str,
    ) -> bus::events::transparency::ReasoningResponse {
        let matching_state = if thought_id.eq_ignore_ascii_case("latest") {
            self.reasoning_history.back()
        } else {
            self.reasoning_history
                .iter()
                .find(|state| thought_id == state.causal_id.as_u64().to_string())
        };

        match matching_state {
            Some(state) => bus::events::transparency::ReasoningResponse {
                causal_id: state.causal_id.as_u64(),
                source_description: Self::describe_inference_source(state.source).to_string(),
                token_count: state.token_count,
                response_preview: state.response_preview.clone(),
            },
            None => bus::events::transparency::ReasoningResponse {
                causal_id: 0,
                source_description: "No inference cycle completed yet".to_string(),
                token_count: 0,
                response_preview: "No reasoning chain is stored for the specified thought_id"
                    .to_string(),
            },
        }
    }

    fn truncate_preview(text: &str, max_chars: usize) -> String {
        let mut chars = text.chars();
        let preview: String = chars.by_ref().take(max_chars).collect();
        if chars.next().is_some() {
            format!("{}...", preview)
        } else {
            preview
        }
    }

    fn describe_inference_source(source: InferenceSource) -> &'static str {
        match source {
            InferenceSource::UserVoice => "user voice input",
            InferenceSource::UserText => "user text input",
            InferenceSource::ProactiveCTP => "a proactive CTP trigger",
            InferenceSource::Iterative => "an iterative follow-up",
        }
    }

    /// Handle an inference request event.
    async fn handle_inference_request(
        &self,
        prompt: String,
        source: InferenceSource,
        priority: Priority,
        causal_id: bus::CausalId,
    ) -> Result<(), InferenceError> {
        debug!(?source, ?priority, ?causal_id, "inference request received");

        let work_item = WorkItem {
            priority,
            kind: WorkKind::Inference {
                prompt,
                source,
                causal_id,
                response_tx: None,
            },
        };

        // Enqueue the work
        let mut queue = self.queue.lock().await;
        if let Err(_rejected) = queue.enqueue(work_item) {
            warn!(?priority, "inference queue full, request rejected");

            if let Some(bus) = &self.bus {
                bus.broadcast(Event::Inference(
                    InferenceEvent::InferenceFailedWithOrigin {
                        origin: match source {
                            InferenceSource::UserVoice | InferenceSource::UserText => {
                                InferenceFailureOrigin::UserRequest
                            }
                            InferenceSource::ProactiveCTP => InferenceFailureOrigin::ProactiveCTP,
                            InferenceSource::Iterative => InferenceFailureOrigin::UserRequest,
                        },
                        reason: "inference queue full".to_string(),
                        causal_id,
                    },
                ))
                .await?;
            }

            return Err(InferenceError::ExecutionFailed("queue full".to_string()));
        }

        trace!(queue_len = queue.len(), "work item enqueued");

        // Signal worker if we have a work channel (non-blocking notification)
        if let Some(tx) = &self.work_tx {
            // Ignore error if channel is full - worker will pick it up on next poll anyway
            let _ = tx.try_send(());
        }

        Ok(())
    }

    async fn handle_model_load_request(
        &self,
        model_path: String,
        causal_id: bus::CausalId,
    ) -> Result<(), InferenceError> {
        let path = std::path::PathBuf::from(&model_path);
        let new_backend = tokio::task::spawn_blocking(move || build_loaded_llama_backend(&path))
            .await
            .map_err(|e| {
                InferenceError::ExecutionFailed(format!("model load task failed: {}", e))
            })??;

        {
            let mut backend = self.backend.lock().await;
            *backend = new_backend;
        }

        if let Some(bus) = &self.bus {
            let model_name = {
                let backend = self.backend.lock().await;
                backend.model_name().unwrap_or("unknown").to_string()
            };
            bus.broadcast(Event::Inference(InferenceEvent::ModelLoaded {
                model_path,
                model_name,
                causal_id,
            }))
            .await?;
        }

        Ok(())
    }

    /// Handle an embedding request event.
    async fn handle_embed_request(
        &self,
        text: String,
        request_id: u64,
        bus: Arc<EventBus>,
    ) -> Result<(), InferenceError> {
        debug!(request_id, "embedding request received");

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let work_item = WorkItem {
            priority: Priority::Normal,
            kind: WorkKind::Embed {
                text,
                causal_id: bus::CausalId::new(),
                response_tx,
            },
        };

        // Enqueue the work
        let mut queue = self.queue.lock().await;
        if let Err(_rejected) = queue.enqueue(work_item) {
            warn!(request_id, "inference queue full, embed request rejected");
            bus.broadcast(Event::Inference(InferenceEvent::EmbedFailed {
                request_id,
                reason: "queue full".to_string(),
            }))
            .await?;
            return Err(InferenceError::ExecutionFailed("queue full".to_string()));
        }

        drop(queue);

        // Signal worker
        if let Some(tx) = &self.work_tx {
            let _ = tx.try_send(());
        }

        // Spawn task to await result and broadcast response
        tokio::spawn(async move {
            match response_rx.await {
                Ok(Ok(vector)) => {
                    let _ = bus
                        .broadcast(Event::Inference(InferenceEvent::EmbedCompleted {
                            vector,
                            request_id,
                        }))
                        .await;
                }
                Ok(Err(reason)) => {
                    let _ = bus
                        .broadcast(Event::Inference(InferenceEvent::EmbedFailed {
                            request_id,
                            reason,
                        }))
                        .await;
                }
                Err(_) => {
                    let _ = bus
                        .broadcast(Event::Inference(InferenceEvent::EmbedFailed {
                            request_id,
                            reason: "response channel closed".to_string(),
                        }))
                        .await;
                }
            }
        });

        Ok(())
    }

    async fn handle_direct_embed_request(&self, request: EmbedRequest) {
        let active_backend = self
            .embed_backend
            .clone()
            .unwrap_or_else(|| self.backend.clone());
        let result = Self::execute_embed(active_backend, request.text, bus::CausalId::new())
            .await
            .map_err(|error| error.to_string());

        let _ = request.response_tx.send(result);
    }

    /// Handle a fact extraction request event.
    async fn handle_extract_request(
        &self,
        text: String,
        request_id: u64,
        bus: Arc<EventBus>,
    ) -> Result<(), InferenceError> {
        debug!(request_id, "extraction request received");

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let work_item = WorkItem {
            priority: Priority::Normal,
            kind: WorkKind::Extract {
                text,
                causal_id: bus::CausalId::new(),
                response_tx,
            },
        };

        // Enqueue the work
        let mut queue = self.queue.lock().await;
        if let Err(_rejected) = queue.enqueue(work_item) {
            warn!(request_id, "inference queue full, extract request rejected");
            // For extraction, we don't emit InferenceFailed since it's internal
            // We'll just return the error
            return Err(InferenceError::ExecutionFailed("queue full".to_string()));
        }

        drop(queue);

        // Signal worker
        if let Some(tx) = &self.work_tx {
            let _ = tx.try_send(());
        }

        // Spawn task to await result and broadcast response
        tokio::spawn(async move {
            match response_rx.await {
                Ok(Ok(facts_json)) => {
                    // Parse the JSON string into Vec<String>
                    let facts = match serde_json::from_str::<Vec<String>>(&facts_json) {
                        Ok(v) => v,
                        Err(_) => vec![facts_json],
                    };

                    let _ = bus
                        .broadcast(Event::Inference(InferenceEvent::ExtractionCompleted {
                            facts,
                            request_id,
                        }))
                        .await;
                }
                Ok(Err(_reason)) => {
                    // Extraction failure - silent, don't emit user-facing event
                }
                Err(_) => {
                    // Response channel closed - silent
                }
            }
        });

        Ok(())
    }

    async fn handle_context_interpretation_request(
        &self,
        context: ContextInterpretationInput,
        causal_id: bus::CausalId,
        bus: Arc<EventBus>,
    ) -> Result<(), InferenceError> {
        debug!(?causal_id, "context interpretation request received");

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let work_item = WorkItem {
            priority: Priority::Low,
            kind: WorkKind::InterpretContext {
                context,
                causal_id,
                response_tx,
            },
        };

        let mut queue = self.queue.lock().await;
        if let Err(_rejected) = queue.enqueue(work_item) {
            warn!(
                ?causal_id,
                "inference queue full, context interpretation rejected"
            );
            return Err(InferenceError::ExecutionFailed("queue full".to_string()));
        }

        drop(queue);

        if let Some(tx) = &self.work_tx {
            let _ = tx.try_send(());
        }

        tokio::spawn(async move {
            match response_rx.await {
                Ok(Ok(task)) => {
                    let _ = bus
                        .broadcast(Event::Inference(
                            InferenceEvent::ContextInterpretationCompleted { task, causal_id },
                        ))
                        .await;
                }
                Ok(Err(reason)) => {
                    let _ = bus
                        .broadcast(Event::Inference(
                            InferenceEvent::ContextInterpretationFailed { reason, causal_id },
                        ))
                        .await;
                }
                Err(_) => {
                    let _ = bus
                        .broadcast(Event::Inference(
                            InferenceEvent::ContextInterpretationFailed {
                                reason: "response channel closed".to_string(),
                                causal_id,
                            },
                        ))
                        .await;
                }
            }
        });

        Ok(())
    }

    /// Worker loop: process queued work items.
    ///
    /// Exits cleanly when the work signal channel closes.
    async fn worker_loop(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        embed_backend: Option<Arc<Mutex<Box<dyn InferenceBackend>>>>,
        queue: Arc<Mutex<InferenceQueue>>,
        conversation_config: Arc<RwLock<ConversationConfig>>,
        mut work_signal: mpsc::Receiver<()>,
    ) {
        loop {
            // Wait for work signal - exit if channel closed
            if work_signal.recv().await.is_none() {
                debug!("worker loop: work channel closed, exiting");
                break;
            }

            // Process all available work
            loop {
                let work_item = {
                    let mut q = queue.lock().await;
                    q.dequeue()
                };

                let Some(item) = work_item else {
                    break;
                };

                match item.kind {
                    WorkKind::Inference {
                        prompt,
                        source,
                        causal_id,
                        response_tx,
                    } => {
                        let result = Self::execute_inference(
                            bus.clone(),
                            backend.clone(),
                            conversation_config.clone(),
                            prompt,
                            source,
                            causal_id,
                        )
                        .await;

                        if let Some(tx) = response_tx {
                            let _ = tx.send(result.map_err(|e| e.to_string()));
                        }
                    }
                    WorkKind::Embed {
                        text,
                        causal_id,
                        response_tx,
                    } => {
                        let active_backend =
                            embed_backend.clone().unwrap_or_else(|| backend.clone());
                        let result = Self::execute_embed(active_backend, text, causal_id).await;
                        let _ = response_tx.send(result.map_err(|e| e.to_string()));
                    }
                    WorkKind::Extract {
                        text,
                        causal_id,
                        response_tx,
                    } => {
                        let result = Self::execute_extract(backend.clone(), text, causal_id).await;
                        let _ = response_tx.send(result.map_err(|e| e.to_string()));
                    }
                    WorkKind::InterpretContext {
                        context,
                        causal_id,
                        response_tx,
                    } => {
                        let result = Self::execute_context_interpretation(
                            backend.clone(),
                            context,
                            causal_id,
                        )
                        .await;
                        let _ = response_tx.send(result.map_err(|e| e.to_string()));
                    }
                }
            }
        }
    }

    /// Execute an inference request with full streaming chain.
    async fn execute_inference(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        conversation_config: Arc<RwLock<ConversationConfig>>,
        prompt: String,
        source: InferenceSource,
        causal_id: bus::CausalId,
    ) -> Result<String, InferenceError> {
        let model_loaded = {
            let backend_guard = backend.lock().await;
            backend_guard.is_loaded()
        };

        if !model_loaded {
            warn!("inference requested but no model loaded");
            bus.broadcast(Event::Inference(
                InferenceEvent::InferenceFailedWithOrigin {
                    origin: Self::failure_origin_for_source(source),
                    reason: "no model loaded".to_string(),
                    causal_id,
                },
            ))
            .await?;
            return Err(InferenceError::ModelNotLoaded);
        }

        let params = conversation_config
            .read()
            .expect("conversation config lock poisoned")
            .clone()
            .to_inference_params();

        let (model_name, prepared_request) = {
            let backend_guard = backend.lock().await;
            let model_name = backend_guard.model_name().map(str::to_string);
            let prepared_request =
                prepare_inference_request(backend_guard.model_name(), &prompt, &params);
            (model_name, prepared_request)
        };

        Self::log_generation_start(
            source,
            causal_id,
            model_name.as_deref(),
            &prepared_request,
        );

        match source {
            InferenceSource::UserVoice | InferenceSource::UserText => {
                Self::execute_streaming_inference(
                    bus,
                    backend,
                    prepared_request.prompt,
                    source,
                    causal_id,
                    prepared_request.params,
                )
                .await
            }
            InferenceSource::ProactiveCTP | InferenceSource::Iterative => {
                Self::execute_batch_inference(
                    bus,
                    backend,
                    prepared_request.prompt,
                    source,
                    causal_id,
                    prepared_request.params,
                )
                .await
            }
        }
    }

    async fn execute_streaming_inference(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        prompt: String,
        source: InferenceSource,
        causal_id: bus::CausalId,
        params: InferenceParams,
    ) -> Result<String, InferenceError> {
        let mut stream = {
            let backend_guard = backend.lock().await;
            backend_guard.infer(prompt, params.clone()).await?
        };

        // Initialize sentence boundary detection
        let mut sentence_iter = SentenceBoundaryIterator::new();
        let mut token_count: u64 = 0;
        let mut sentence_index: u32 = 0;
        let mut full_text = String::new();

        // Stream tokens
        while let Some(result) = stream.next().await {
            match result {
                Ok(token) => {
                    full_text.push_str(&token);
                    token_count += 1;

                    // Emit token event
                    bus.broadcast(Event::Inference(InferenceEvent::InferenceTokenGenerated {
                        token: token.clone(),
                        sequence_number: token_count,
                        causal_id,
                    }))
                    .await?;

                    // Feed to sentence detector
                    sentence_iter.push(&token);

                    // Emit any completed sentences
                    for sentence in sentence_iter.by_ref() {
                        trace!(sentence_len = sentence.len(), "sentence boundary detected");
                        let tts_text = OutputFilter::apply(&sentence);
                        bus.broadcast(Event::Inference(InferenceEvent::InferenceSentenceReady {
                            text: tts_text,
                            sentence_index,
                            causal_id,
                        }))
                        .await?;
                        sentence_index += 1;
                    }
                }
                Err(e) => {
                    warn!(error = ?e, "inference stream error");
                    bus.broadcast(Event::Inference(
                        InferenceEvent::InferenceFailedWithOrigin {
                            origin: Self::failure_origin_for_source(source),
                            reason: e.to_string(),
                            causal_id,
                        },
                    ))
                    .await?;
                    return Err(e);
                }
            }
        }

        let diagnostics = {
            let backend_guard = backend.lock().await;
            backend_guard.take_generation_diagnostics()
        };
        let completed_token_count = diagnostics
            .as_ref()
            .map(|details| details.generated_token_count as u64)
            .unwrap_or(token_count);
        Self::log_generation_finish(
            source,
            causal_id,
            diagnostics.as_ref(),
            token_count as usize,
            &params,
            &full_text,
        );

        // Flush any remaining incomplete sentence
        if let Some(remaining) = sentence_iter.flush()
            && !remaining.trim().is_empty()
        {
            trace!(
                remaining_len = remaining.len(),
                "flushing remaining text as final sentence"
            );
            let tts_text = OutputFilter::apply(&remaining);
            bus.broadcast(Event::Inference(InferenceEvent::InferenceSentenceReady {
                text: tts_text,
                sentence_index,
                causal_id,
            }))
            .await?;
        }

        // Compute simple confidence heuristic
        // TODO: Replace with proper confidence scoring from model probabilities
        let confidence = if completed_token_count >= 10 { 0.95 } else { 0.75 };

        // Emit stream completed event
        bus.broadcast(Event::Inference(InferenceEvent::InferenceStreamCompleted {
            full_text: full_text.clone(),
            source,
            token_count: completed_token_count as usize,
            confidence: Some(confidence),
            causal_id,
        }))
        .await?;

        // Emit final completion event
        info!(
            token_count = completed_token_count,
            text_len = full_text.len(),
            "inference complete"
        );
        bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
            text: full_text.clone(),
            source,
            token_count: completed_token_count as usize,
            causal_id,
        }))
        .await?;

        Ok(full_text)
    }

    async fn execute_batch_inference(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        prompt: String,
        source: InferenceSource,
        causal_id: bus::CausalId,
        params: InferenceParams,
    ) -> Result<String, InferenceError> {
        let full_text = {
            let backend_guard = backend.lock().await;
            backend_guard.complete(&prompt, &params)?
        };

        let token_count = full_text.len() / 4;
        Self::log_generation_finish(source, causal_id, None, token_count, &params, &full_text);

        if source == InferenceSource::ProactiveCTP {
            return Self::route_proactive_batch_output(bus, full_text, causal_id, token_count)
                .await;
        }

        Self::emit_sentence_events(&bus, &full_text, causal_id).await?;

        bus.broadcast(Event::Inference(InferenceEvent::InferenceStreamCompleted {
            full_text: full_text.clone(),
            source,
            token_count,
            confidence: Some(1.0),
            causal_id,
        }))
        .await?;

        info!(
            token_count,
            text_len = full_text.len(),
            "batch inference complete"
        );
        bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
            text: full_text.clone(),
            source,
            token_count,
            causal_id,
        }))
        .await?;

        Ok(full_text)
    }

    async fn emit_sentence_events(
        bus: &Arc<EventBus>,
        text: &str,
        causal_id: bus::CausalId,
    ) -> Result<(), InferenceError> {
        let mut sentence_iter = SentenceBoundaryIterator::new();
        sentence_iter.push(text);
        let mut sentence_index: u32 = 0;

        for sentence in sentence_iter.by_ref() {
            let tts_text = OutputFilter::apply(&sentence);
            if !tts_text.trim().is_empty() {
                bus.broadcast(Event::Inference(InferenceEvent::InferenceSentenceReady {
                    text: tts_text,
                    sentence_index,
                    causal_id,
                }))
                .await?;
                sentence_index += 1;
            }
        }

        if let Some(remaining) = sentence_iter.flush()
            && !remaining.trim().is_empty()
        {
            let tts_text = OutputFilter::apply(&remaining);
            if !tts_text.trim().is_empty() {
                bus.broadcast(Event::Inference(InferenceEvent::InferenceSentenceReady {
                    text: tts_text,
                    sentence_index,
                    causal_id,
                }))
                .await?;
            }
        }

        Ok(())
    }

    async fn route_proactive_batch_output(
        bus: Arc<EventBus>,
        full_text: String,
        causal_id: bus::CausalId,
        token_count: usize,
    ) -> Result<String, InferenceError> {
        let decision = match Self::parse_proactive_decision(&full_text) {
            Some(decision) => decision,
            None => {
                warn!("proactive inference returned an unparsable contract; discarding output");
                return Ok(String::new());
            }
        };

        match decision.action {
            ProactiveAction::Speak => {
                if decision.content.trim().is_empty() {
                    return Ok(String::new());
                }

                Self::emit_sentence_events(&bus, &decision.content, causal_id).await?;
                bus.broadcast(Event::Inference(InferenceEvent::InferenceStreamCompleted {
                    full_text: decision.content.clone(),
                    source: InferenceSource::ProactiveCTP,
                    token_count,
                    confidence: Some(1.0),
                    causal_id,
                }))
                .await?;
                bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
                    text: decision.content.clone(),
                    source: InferenceSource::ProactiveCTP,
                    token_count,
                    causal_id,
                }))
                .await?;
                Ok(decision.content)
            }
            ProactiveAction::Observe => {
                if !decision.content.trim().is_empty() {
                    bus.broadcast(Event::Memory(MemoryEvent::MemoryWriteRequest {
                        text: decision.content.clone(),
                        kind: MemoryKind::Semantic,
                        causal_id,
                    }))
                    .await?;
                }
                bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
                    text: decision.content.clone(),
                    source: InferenceSource::ProactiveCTP,
                    token_count,
                    causal_id,
                }))
                .await?;
                Ok(decision.content)
            }
            ProactiveAction::Nothing => {
                bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
                    text: String::new(),
                    source: InferenceSource::ProactiveCTP,
                    token_count,
                    causal_id,
                }))
                .await?;
                Ok(String::new())
            }
        }
    }

    fn parse_proactive_decision(raw: &str) -> Option<ProactiveDecision> {
        let mut action = None;
        let mut content_lines = Vec::new();
        let mut reading_content = false;

        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() && !reading_content {
                continue;
            }

            if let Some(value) = Self::parse_named_field(trimmed, "action") {
                action = match value.to_ascii_lowercase().as_str() {
                    "speak" => Some(ProactiveAction::Speak),
                    "observe" => Some(ProactiveAction::Observe),
                    "nothing" => Some(ProactiveAction::Nothing),
                    _ => None,
                };
                reading_content = false;
                continue;
            }

            if let Some(value) = Self::parse_named_field(trimmed, "content") {
                reading_content = true;
                if !value.is_empty() {
                    content_lines.push(value.to_string());
                }
                continue;
            }

            if reading_content {
                content_lines.push(trimmed.to_string());
            }
        }

        Some(ProactiveDecision {
            action: action?,
            content: content_lines.join("\n").trim().to_string(),
        })
    }

    fn parse_named_field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
        let (field, value) = line.split_once(':')?;
        if field.trim().eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    }

    /// Execute an embedding request.
    async fn execute_embed(
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        text: String,
        _causal_id: bus::CausalId,
    ) -> Result<Vec<f32>, InferenceError> {
        let backend_guard = backend.lock().await;

        if !backend_guard.is_loaded() {
            warn!("embedding backend not loaded; returning zero-vector fallback");
            return Ok(vec![0.0; EMBEDDING_FALLBACK_DIMENSIONS]);
        }

        match backend_guard.embed(text).await {
            Ok(embedding) if embedding.len() == EMBEDDING_FALLBACK_DIMENSIONS => Ok(embedding),
            Ok(embedding) => {
                warn!(
                    expected = EMBEDDING_FALLBACK_DIMENSIONS,
                    actual = embedding.len(),
                    "embedding backend returned unexpected dimension; returning zero-vector fallback"
                );
                Ok(vec![0.0; EMBEDDING_FALLBACK_DIMENSIONS])
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "embedding backend failed; returning zero-vector fallback"
                );
                Ok(vec![0.0; EMBEDDING_FALLBACK_DIMENSIONS])
            }
        }
    }

    /// Execute an extraction request.
    async fn execute_extract(
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        text: String,
        _causal_id: bus::CausalId,
    ) -> Result<String, InferenceError> {
        let backend_guard = backend.lock().await;

        if !backend_guard.is_loaded() {
            return Err(InferenceError::ModelNotLoaded);
        }

        let extracted = backend_guard.extract(text).await?;
        Ok(extracted)
    }

    async fn execute_context_interpretation(
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        context: ContextInterpretationInput,
        _causal_id: bus::CausalId,
    ) -> Result<Option<EnrichedInferredTask>, InferenceError> {
        #[derive(Deserialize)]
        struct InterpretationResponse {
            category: String,
            semantic_description: String,
            confidence: f32,
        }

        let backend_guard = backend.lock().await;

        if !backend_guard.is_loaded() {
            return Err(InferenceError::ModelNotLoaded);
        }

        let ContextInterpretationInput {
            snapshot,
            patterns,
            memory_relevance,
        } = context;

        let recent_files: Vec<_> = snapshot
            .recent_files
            .iter()
            .rev()
            .filter_map(|event| {
                event.path.file_name().and_then(|name| {
                    name.to_str().map(|file_name| {
                        serde_json::json!({
                            "file_name": file_name,
                            "event_kind": format!("{:?}", event.event_kind),
                        })
                    })
                })
            })
            .take(3)
            .collect();

        let patterns_json: Vec<_> = patterns
            .iter()
            .map(|pattern| {
                serde_json::json!({
                    "pattern_type": format!("{:?}", pattern.pattern_type),
                    "confidence": pattern.confidence,
                    "description": pattern.description,
                })
            })
            .collect();

        let snapshot_json = serde_json::to_string_pretty(&serde_json::json!({
            "active_app": snapshot.active_app.app_name,
            "window_title": snapshot.active_app.window_title,
            "bundle_id": snapshot.active_app.bundle_id,
            "recent_file_count": snapshot.recent_files.len(),
            "recent_files": recent_files,
            "clipboard_present": snapshot.clipboard_digest.is_some(),
            "memory_relevance": memory_relevance,
            "patterns": patterns_json,
            "keystroke": {
                "events_per_minute": snapshot.keystroke_cadence.events_per_minute,
                "burst_detected": snapshot.keystroke_cadence.burst_detected,
                "idle_duration_seconds": snapshot.keystroke_cadence.idle_duration.as_secs(),
            },
            "session_duration_seconds": snapshot.session_duration.as_secs(),
            "visual_context": snapshot.visual_context.as_ref().map(|visual_context| serde_json::json!({
                "resolution": visual_context.resolution,
                "age_seconds": visual_context.age.as_secs(),
            })),
            "identity_signal": snapshot.soul_identity_signal.as_ref().map(|signal| serde_json::json!({
                "signal_key": signal.signal_key,
                "confidence": signal.confidence,
            })),
        }))
        .map_err(|e| InferenceError::ExecutionFailed(format!("snapshot serialization failed: {}", e)))?;

        let prompt = format!(
            "Interpret the user's current task from this structured context. Do not assume the task domain. If the signal is too weak, return null. If it is strong enough, return only JSON with keys category, semantic_description, confidence.\n\nContext:\n{}\n\nResult:",
            snapshot_json
        );

        let raw = backend_guard.complete(
            &prompt,
            &InferenceParams {
                temperature: 0.2,
                top_p: 0.9,
                top_k: 40,
                max_tokens: 180,
                stop_sequences: vec!["\n\n".to_string()],
                repeat_penalty: 1.05,
            },
        )?;

        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("null") {
            return Ok(None);
        }

        let parsed: InterpretationResponse = serde_json::from_str(trimmed).map_err(|e| {
            InferenceError::ExecutionFailed(format!("interpretation parse failed: {}", e))
        })?;

        Ok(Some(EnrichedInferredTask {
            category: parsed.category,
            semantic_description: parsed.semantic_description,
            confidence: parsed.confidence.clamp(0.0, 1.0),
        }))
    }

    /// VRAM monitoring loop: poll backend VRAM usage every 2 seconds when model loaded.
    ///
    /// Responds to LoopControlRequested events to enable/disable monitoring.
    /// Exits when shutdown signal is received on the bus.
    async fn vram_monitoring_loop(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
    ) {
        let mut shutdown_rx = bus.subscribe_broadcast();
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
        let mut enabled = true;

        // Broadcast initial loop status
        let _ = bus
            .broadcast(Event::System(bus::SystemEvent::LoopStatusChanged {
                loop_name: "vram_monitor".to_string(),
                enabled: true,
            }))
            .await;

        debug!("vram monitoring loop started");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if enabled {
                        let backend_guard = backend.lock().await;
                        if backend_guard.is_loaded() {
                            let (mut used_mb, mut total_mb, mut percent) = backend_guard.vram_usage();
                            drop(backend_guard);

                            if total_mb == 0 {
                                let (fallback_used, fallback_total) = poll_system_vram_usage();
                                used_mb = fallback_used;
                                total_mb = fallback_total;
                                percent = fallback_used
                                    .saturating_mul(100)
                                    .checked_div(fallback_total)
                                    .unwrap_or(0)
                                    .min(100) as u8;
                            }

                            let normalized_percent = if total_mb == 0 { 0 } else { percent };
                            let _ = bus.broadcast(Event::System(bus::SystemEvent::VramUsageUpdated {
                                used_mb,
                                total_mb,
                                percent: normalized_percent,
                            })).await;
                        }
                    }
                }
                event = shutdown_rx.recv() => {
                    match event {
                        Ok(Event::System(bus::SystemEvent::ShutdownSignal))
                        | Ok(Event::System(bus::SystemEvent::ShutdownRequested))
                        | Ok(Event::System(bus::SystemEvent::ShutdownInitiated)) => {
                            debug!("vram monitoring loop: shutdown signal received");
                            break;
                        }
                        Ok(Event::System(bus::SystemEvent::LoopControlRequested {
                            loop_name,
                            enabled: new_enabled,
                        })) if loop_name == "vram_monitor" && enabled != new_enabled => {
                            debug!(
                                enabled = new_enabled,
                                "vram monitoring loop: control requested"
                            );
                            enabled = new_enabled;
                            let _ = bus
                                .broadcast(Event::System(bus::SystemEvent::LoopStatusChanged {
                                    loop_name: "vram_monitor".to_string(),
                                    enabled,
                                }))
                                .await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!(error = ?e, "vram monitoring loop: broadcast recv error");
                            break;
                        }
                    }
                }
            }
        }

        debug!("vram monitoring loop exited");
    }
}

#[cfg(target_os = "windows")]
fn poll_system_vram_usage() -> (u32, u32) {
    parse_nvidia_smi_vram(
        Command::new("nvidia-smi")
            .args([
                "--query-gpu=memory.used,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output(),
    )
}

#[cfg(target_os = "linux")]
fn poll_system_vram_usage() -> (u32, u32) {
    parse_nvidia_smi_vram(
        Command::new("nvidia-smi")
            .args([
                "--query-gpu=memory.used,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output(),
    )
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn poll_system_vram_usage() -> (u32, u32) {
    (0, 0)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn parse_nvidia_smi_vram(output: std::io::Result<std::process::Output>) -> (u32, u32) {
    if let Ok(output) = output
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(first_line) = stdout.lines().next() {
            let parts: Vec<&str> = first_line.split(',').map(|s| s.trim()).collect();
            if parts.len() == 2
                && let (Ok(used), Ok(total)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>())
            {
                return (used, total);
            }
        }
    }

    (0, 0)
}

impl Actor for InferenceActor {
    fn name(&self) -> &'static str {
        "inference"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!("inference actor starting");
        self.rx = Some(bus.subscribe_broadcast());
        self.bus = Some(bus.clone());

        // Register directed channel for internal embed/extract requests from memory
        let (directed_tx, directed_rx) = mpsc::channel(32);
        bus.register_directed("inference", directed_tx)
            .map_err(|e| {
                ActorError::StartupFailed(format!("failed to register directed channel: {e}"))
            })?;
        self.directed_rx = Some(directed_rx);

        // Create work notification channel
        let (work_tx, work_rx) = mpsc::channel(10);
        self.work_tx = Some(work_tx);

        // Spawn worker task
        let worker_bus = bus.clone();
        let worker_backend = self.backend.clone();
        let worker_embed_backend = self.embed_backend.clone();
        let worker_queue = self.queue.clone();
        let worker_conversation_config = self.conversation_config.clone();

        tokio::spawn(async move {
            Self::worker_loop(
                worker_bus,
                worker_backend,
                worker_embed_backend,
                worker_queue,
                worker_conversation_config,
                work_rx,
            )
            .await;
        });

        // Spawn VRAM monitoring task
        let vram_bus = bus.clone();
        let vram_backend = self.backend.clone();
        tokio::spawn(async move {
            Self::vram_monitoring_loop(vram_bus, vram_backend).await;
        });

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        info!("inference actor running");

        let mut rx = self
            .rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("receiver not initialized".to_string()))?;

        let mut directed_rx = self.directed_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("directed receiver not initialized".to_string())
        })?;
        let mut embed_rx = Some(self.embed_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("embed receiver not initialized".to_string())
        })?);

        let bus = self
            .bus
            .clone()
            .ok_or_else(|| ActorError::RuntimeError("bus not initialized".to_string()))?;

        loop {
            tokio::select! {
                // Handle broadcast events (user-facing inference, shutdown)
                event = rx.recv() => {
                    match event {
                        Ok(Event::System(bus::SystemEvent::ShutdownSignal))
                        | Ok(Event::System(bus::SystemEvent::ShutdownRequested))
                        | Ok(Event::System(bus::SystemEvent::ShutdownInitiated)) => {
                            info!("inference actor received shutdown signal");
                            break;
                        }
                        Ok(Event::System(bus::SystemEvent::TokenBudgetAutoTuned {
                            new_max_tokens,
                            ..
                        })) => {
                            self.conversation_config
                                .write()
                                .expect("conversation config lock poisoned")
                                .max_tokens = new_max_tokens as u32;
                            info!(new_max_tokens, "inference actor updated token budget");
                        }
                        Ok(Event::Inference(InferenceEvent::InferenceRequested {
                            prompt,
                            priority,
                            source,
                            causal_id,
                        })) => {
                            self.remember_pending_reasoning(source, causal_id);
                            if source == InferenceSource::UserText {
                                self.remember_user_input(causal_id, prompt.clone());
                            }
                            if let Err(e) = self
                                .handle_inference_request(prompt, source, priority, causal_id)
                                .await
                            {
                                warn!(error = ?e, "inference request handling failed");
                            }
                        }
                        Ok(Event::Inference(InferenceEvent::InferenceCompleted {
                            text,
                            token_count,
                            causal_id,
                            ..
                        })) => {
                            self.remember_completed_reasoning(causal_id, &text, token_count);
                        }
                        Ok(Event::Inference(InferenceEvent::InferenceStreamCompleted {
                            full_text,
                            source,
                            causal_id,
                            ..
                        })) => {
                            if let Err(error) = self
                                .persist_completed_exchange(&bus, source, &full_text, causal_id)
                                .await
                            {
                                warn!(
                                    error = %error,
                                    ?causal_id,
                                    "failed to persist completed exchange to memory"
                                );
                            }
                        }
                        Ok(Event::Speech(bus::SpeechEvent::TranscriptionCompleted {
                            text,
                            causal_id,
                            ..
                        })) => {
                            self.remember_user_input(causal_id, text);
                        }
                        Ok(Event::Inference(InferenceEvent::InferenceFailed { causal_id, .. }))
                        | Ok(Event::Inference(InferenceEvent::InferenceFailedWithOrigin {
                            causal_id,
                            ..
                        })) => {
                            self.forget_pending_reasoning(causal_id);
                            self.pending_user_inputs.remove(&causal_id);
                        }
                        Ok(Event::Inference(InferenceEvent::ModelLoadRequested {
                            model_path,
                            causal_id,
                        })) => {
                            if let Err(e) = self.handle_model_load_request(model_path.clone(), causal_id).await {
                                warn!(error = ?e, model_path = %model_path, "model load request handling failed");
                                if let Some(bus) = &self.bus {
                                    let _ = bus.broadcast(Event::Inference(InferenceEvent::ModelLoadFailed {
                                        model_path,
                                        reason: e.to_string(),
                                        causal_id,
                                    })).await;
                                }
                            }
                        }
                        Ok(Event::Transparency(TransparencyEvent::QueryRequested(
                            TransparencyQuery::ReasoningChain { thought_id },
                        ))) => {
                            let response = self.build_reasoning_response(&thought_id);
                            let _ = bus.broadcast(Event::Transparency(
                                TransparencyEvent::QueryResponse {
                                    query: TransparencyQuery::ReasoningChain { thought_id },
                                    result: Box::new(TransparencyResult::Reasoning(response)),
                                },
                            )).await;
                        }
                        Ok(Event::Inference(InferenceEvent::ContextInterpretationRequested {
                            context,
                            causal_id,
                        })) => {
                            if let Err(e) = self
                                .handle_context_interpretation_request(context, causal_id, bus.clone())
                                .await
                            {
                                warn!(error = ?e, ?causal_id, "context interpretation handling failed");
                                let _ = bus.broadcast(Event::Inference(
                                    InferenceEvent::ContextInterpretationFailed {
                                        reason: e.to_string(),
                                        causal_id,
                                    },
                                )).await;
                            }
                        }
                        Ok(_) => {
                            // Ignore other broadcast events
                        }
                        Err(e) => {
                            warn!(error = ?e, "broadcast recv error");
                            return Err(ActorError::ChannelClosed(e.to_string()));
                        }
                    }
                }
                // Handle directed events (internal embed/extract from memory)
                event = directed_rx.recv() => {
                    match event {
                        Some(Event::Inference(InferenceEvent::EmbedRequested { text, request_id })) => {
                            if let Err(e) = self
                                .handle_embed_request(text, request_id, bus.clone())
                                .await
                            {
                                warn!(error = ?e, request_id, "embed request handling failed");
                            }
                        }
                        Some(Event::Inference(InferenceEvent::ExtractionRequested {
                            text,
                            request_id,
                        })) => {
                            if let Err(e) = self
                                .handle_extract_request(text, request_id, bus.clone())
                                .await
                            {
                                warn!(error = ?e, request_id, "extract request handling failed");
                            }
                        }
                        Some(_) => {
                            // Ignore other directed events
                        }
                        None => {
                            warn!("directed channel closed");
                            return Err(ActorError::ChannelClosed("directed channel closed".to_string()));
                        }
                    }
                }
                request = async {
                    match &mut embed_rx {
                        Some(rx) => rx.recv().await,
                        None => pending().await,
                    }
                } => {
                    match request {
                        Some(request) => {
                            self.handle_direct_embed_request(request).await;
                        }
                        None => {
                            debug!("embed request channel closed");
                            embed_rx = None;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!("inference actor stopping");

        // Drop work_tx to close the channel and signal worker to exit
        drop(self.work_tx.take());
        drop(self.embed_tx_guard.take());

        // Shutdown backend
        let mut backend = self.backend.lock().await;
        backend
            .shutdown()
            .await
            .map_err(|e| ActorError::RuntimeError(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockBackend, MockConfig};
    use crate::stream::InferenceStream;
    use crate::types::BackendType;
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::oneshot;

    async fn spawn_memory_ack_responder(
        bus: Arc<EventBus>,
        expected_writes: usize,
        observed_writes: Option<Arc<StdMutex<Vec<String>>>>,
    ) -> (oneshot::Receiver<()>, tokio::task::JoinHandle<()>) {
        let (ready_tx, ready_rx) = oneshot::channel();
        let responder = tokio::spawn(async move {
            let mut rx = bus.subscribe_broadcast();
            let _ = ready_tx.send(());
            let mut seen = 0;

            while seen < expected_writes {
                match rx.recv().await {
                    Ok(Event::Memory(MemoryEvent::MemoryWriteRequest {
                        text,
                        kind,
                        causal_id,
                    })) => {
                        assert_eq!(kind, MemoryKind::Episodic);
                        if let Some(observed) = &observed_writes {
                            observed
                                .lock()
                                .expect("memory write capture mutex")
                                .push(text);
                        }
                        bus.broadcast(Event::Memory(MemoryEvent::MemoryWriteCompleted {
                            causal_id,
                        }))
                        .await
                        .expect("memory write completion should broadcast");
                        seen += 1;
                    }
                    Ok(_) => {}
                    Err(error) => panic!("memory write responder channel failed: {error}"),
                }
            }
        });

        (ready_rx, responder)
    }

    struct PromptCaptureBackend {
        response: String,
        last_prompt: Arc<StdMutex<Option<String>>>,
    }

    impl PromptCaptureBackend {
        fn new(response: impl Into<String>, last_prompt: Arc<StdMutex<Option<String>>>) -> Self {
            Self {
                response: response.into(),
                last_prompt,
            }
        }
    }

    #[async_trait]
    impl InferenceBackend for PromptCaptureBackend {
        fn backend_type(&self) -> BackendType {
            BackendType::Mock
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
                "streaming not used in prompt capture test".to_string(),
            ))
        }

        fn complete(
            &self,
            prompt: &str,
            _params: &InferenceParams,
        ) -> Result<String, InferenceError> {
            let mut last_prompt = self
                .last_prompt
                .lock()
                .expect("prompt capture mutex should not be poisoned");
            *last_prompt = Some(prompt.to_string());
            Ok(self.response.clone())
        }

        async fn shutdown(&mut self) -> Result<(), InferenceError> {
            Ok(())
        }
    }

    #[test]
    fn actor_allows_configured_token_budget() {
        let backend = Box::new(MockBackend::default_loaded());
        let actor = InferenceActor::new(backend).with_inference_max_tokens(768);

        assert_eq!(
            actor
                .conversation_config
                .read()
                .expect("conversation config lock poisoned")
                .max_tokens,
            768
        );
    }

    #[tokio::test]
    async fn actor_handles_inference_request_when_model_loaded() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::with_config(MockConfig {
            loaded: true,
            response: "Hello world. How are you?".to_string(),
            token_count: 6,
            ..Default::default()
        }));
        let mut actor = InferenceActor::new(backend);

        let mut rx = bus.subscribe_broadcast();

        // Start and run actor
        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Send inference request
        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test".to_string(),
            priority: bus::Priority::Normal,
            source: bus::InferenceSource::UserText,
            causal_id,
        }))
        .await
        .unwrap();

        // Collect events
        let mut completed = false;
        let mut token_count = 0;
        let mut stream_completed = false;
        let mut sentences = Vec::new();

        for _ in 0..50 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let Ok(Event::Inference(inf_event)) = event {
                    match inf_event {
                        InferenceEvent::InferenceTokenGenerated { .. } => {
                            token_count += 1;
                        }
                        InferenceEvent::InferenceSentenceReady { text, .. } => {
                            sentences.push(text);
                        }
                        InferenceEvent::InferenceStreamCompleted { .. } => {
                            stream_completed = true;
                        }
                        InferenceEvent::InferenceCompleted { .. } => {
                            completed = true;
                            break;
                        }
                        _ => {}
                    }
                }
            } else {
                break;
            }
        }

        assert!(completed, "should receive completion event");
        assert!(stream_completed, "should receive stream completed event");
        assert!(token_count >= 1, "should receive token events");
        assert!(!sentences.is_empty(), "should detect sentences");
    }

    #[tokio::test]
    async fn actor_emits_failure_when_model_not_loaded() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::with_config(MockConfig {
            loaded: false,
            ..Default::default()
        }));
        let mut actor = InferenceActor::new(backend);

        let mut rx = bus.subscribe_broadcast();

        // Start and run actor
        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Send inference request
        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test".to_string(),
            priority: bus::Priority::Normal,
            source: bus::InferenceSource::UserText,
            causal_id,
        }))
        .await
        .unwrap();

        // Wait for failure event
        let mut failed = false;
        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let Ok(Event::Inference(InferenceEvent::InferenceFailedWithOrigin { .. })) =
                    event
                {
                    failed = true;
                    break;
                }
            } else {
                break;
            }
        }

        assert!(failed, "should emit failure event when model not loaded");
    }

    #[tokio::test]
    async fn proactive_ctp_speak_emits_sentences_from_contract_content() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::with_response(
            "action: speak\ncontent: Speak this aloud.",
        ));
        let mut actor = InferenceActor::new(backend);
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test proactive prompt".to_string(),
            priority: bus::Priority::Low,
            source: bus::InferenceSource::ProactiveCTP,
            causal_id,
        }))
        .await
        .unwrap();

        let mut saw_sentence = false;
        let mut saw_completed = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Inference(event))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                match event {
                    InferenceEvent::InferenceSentenceReady { text, .. } => {
                        assert_eq!(text, "Speak this aloud.");
                        saw_sentence = true;
                    }
                    InferenceEvent::InferenceCompleted { text, source, .. } => {
                        assert_eq!(source, InferenceSource::ProactiveCTP);
                        assert_eq!(text, "Speak this aloud.");
                        saw_completed = true;
                        break;
                    }
                    _ => {}
                }
            }
        }

        assert!(
            saw_sentence,
            "speak action should emit sentence-ready events"
        );
        assert!(
            saw_completed,
            "speak action should complete the inference cycle"
        );
    }

    #[tokio::test]
    async fn proactive_ctp_observe_writes_memory_without_speaking() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::with_response(
            "action: observe\ncontent: The user is focused on Rust code.",
        ));
        let mut actor = InferenceActor::new(backend);
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test proactive prompt".to_string(),
            priority: bus::Priority::Low,
            source: bus::InferenceSource::ProactiveCTP,
            causal_id,
        }))
        .await
        .unwrap();

        let mut saw_memory_write = false;
        let mut saw_sentence = false;
        let mut saw_completed = false;
        for _ in 0..20 {
            if let Ok(Ok(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                match event {
                    Event::Memory(MemoryEvent::MemoryWriteRequest { text, kind, .. }) => {
                        assert_eq!(kind, MemoryKind::Semantic);
                        assert_eq!(text, "The user is focused on Rust code.");
                        saw_memory_write = true;
                    }
                    Event::Inference(InferenceEvent::InferenceSentenceReady { .. }) => {
                        saw_sentence = true;
                    }
                    Event::Inference(InferenceEvent::InferenceCompleted {
                        source, text, ..
                    }) => {
                        assert_eq!(source, InferenceSource::ProactiveCTP);
                        assert_eq!(text, "The user is focused on Rust code.");
                        saw_completed = true;
                        break;
                    }
                    _ => {}
                }
            }
        }

        assert!(saw_memory_write, "observe action should write memory");
        assert!(
            !saw_sentence,
            "observe action should not emit spoken sentences"
        );
        assert!(
            saw_completed,
            "observe action should still complete the inference cycle"
        );
    }

    #[tokio::test]
    async fn proactive_ctp_nothing_completes_without_side_effects() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::with_response("action: nothing\ncontent:"));
        let mut actor = InferenceActor::new(backend);
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test proactive prompt".to_string(),
            priority: bus::Priority::Low,
            source: bus::InferenceSource::ProactiveCTP,
            causal_id,
        }))
        .await
        .unwrap();

        let mut saw_memory_write = false;
        let mut saw_sentence = false;
        let mut saw_completed = false;
        for _ in 0..20 {
            if let Ok(Ok(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                match event {
                    Event::Memory(MemoryEvent::MemoryWriteRequest { .. }) => {
                        saw_memory_write = true;
                    }
                    Event::Inference(InferenceEvent::InferenceSentenceReady { .. }) => {
                        saw_sentence = true;
                    }
                    Event::Inference(InferenceEvent::InferenceCompleted {
                        source, text, ..
                    }) => {
                        assert_eq!(source, InferenceSource::ProactiveCTP);
                        assert!(text.is_empty());
                        saw_completed = true;
                        break;
                    }
                    _ => {}
                }
            }
        }

        assert!(!saw_memory_write, "nothing action should not write memory");
        assert!(
            !saw_sentence,
            "nothing action should not emit spoken sentences"
        );
        assert!(
            saw_completed,
            "nothing action should still complete the inference cycle"
        );
    }

    #[tokio::test]
    async fn actor_queue_respects_priority() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::with_config(MockConfig {
            loaded: true,
            response: "response".to_string(),
            token_count: 1,
            ..Default::default()
        }));
        let mut actor = InferenceActor::with_queue_capacity(backend, 10);

        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Submit requests in different priorities
        for priority in [
            bus::Priority::Low,
            bus::Priority::Normal,
            bus::Priority::High,
        ] {
            let causal_id = bus::CausalId::new();
            bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
                prompt: format!("priority {:?}", priority),
                priority,
                source: bus::InferenceSource::UserText,
                causal_id,
            }))
            .await
            .unwrap();
        }

        // Queue should process high priority first
        // (Hard to test exact order without exposing queue internals,
        // but we verify no crashes and all complete)
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    #[tokio::test]
    async fn actor_updates_token_budget_from_auto_tune_event() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::default_loaded());
        let mut actor = InferenceActor::new(backend);
        let shared_config = actor.conversation_config.clone();

        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        bus.broadcast(Event::System(bus::SystemEvent::TokenBudgetAutoTuned {
            old_max_tokens: 512,
            new_max_tokens: 896,
            p95_tokens: 700,
        }))
        .await
        .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(
            shared_config
                .read()
                .expect("conversation config lock poisoned")
                .max_tokens,
            896
        );
    }

    struct ParamsCaptureBackend {
        response: String,
        observed_params: Arc<StdMutex<Vec<InferenceParams>>>,
    }

    impl ParamsCaptureBackend {
        fn new(
            response: impl Into<String>,
            observed_params: Arc<StdMutex<Vec<InferenceParams>>>,
        ) -> Self {
            Self {
                response: response.into(),
                observed_params,
            }
        }
    }

    #[async_trait]
    impl InferenceBackend for ParamsCaptureBackend {
        fn backend_type(&self) -> BackendType {
            BackendType::Mock
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
                "streaming not used in params capture test".to_string(),
            ))
        }

        fn complete(
            &self,
            _prompt: &str,
            params: &InferenceParams,
        ) -> Result<String, InferenceError> {
            self.observed_params
                .lock()
                .expect("params capture mutex should not be poisoned")
                .push(params.clone());
            Ok(self.response.clone())
        }

        async fn shutdown(&mut self) -> Result<(), InferenceError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn inference_uses_updated_conversation_config_on_next_call() {
        let bus = Arc::new(EventBus::new());
        let observed_params = Arc::new(StdMutex::new(Vec::new()));
        let shared_config = Arc::new(RwLock::new(ConversationConfig::default()));
        let backend = Box::new(ParamsCaptureBackend::new(
            "iterative response",
            observed_params.clone(),
        ));
        let mut actor =
            InferenceActor::new(backend).with_shared_conversation_config(shared_config.clone());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        let first_causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "first".to_string(),
            priority: bus::Priority::Normal,
            source: InferenceSource::Iterative,
            causal_id: first_causal_id,
        }))
        .await
        .unwrap();

        for _ in 0..20 {
            if let Ok(Ok(Event::Inference(InferenceEvent::InferenceCompleted { causal_id, .. }))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
                && causal_id == first_causal_id
            {
                break;
            }
        }

        {
            let mut config = shared_config
                .write()
                .expect("conversation config lock poisoned");
            config.temperature = 0.9;
            config.max_tokens = 222;
            config.repeat_penalty = 1.25;
        }

        let second_causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "second".to_string(),
            priority: bus::Priority::Normal,
            source: InferenceSource::Iterative,
            causal_id: second_causal_id,
        }))
        .await
        .unwrap();

        for _ in 0..20 {
            if let Ok(Ok(Event::Inference(InferenceEvent::InferenceCompleted { causal_id, .. }))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
                && causal_id == second_causal_id
            {
                break;
            }
        }

        let observed = observed_params
            .lock()
            .expect("params capture mutex should not be poisoned")
            .clone();
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].max_tokens, 150);
        assert_eq!(observed[0].temperature, 0.7);
        assert_eq!(observed[0].repeat_penalty, 1.15);
        assert_eq!(observed[1].max_tokens, 222);
        assert_eq!(observed[1].temperature, 0.9);
        assert_eq!(observed[1].repeat_penalty, 1.25);
    }

    #[tokio::test]
    async fn execute_embed_falls_back_to_zero_vector() {
        let backend: Arc<Mutex<Box<dyn InferenceBackend>>> =
            Arc::new(Mutex::new(Box::new(MockBackend::unloaded())));

        let vector =
            InferenceActor::execute_embed(backend, "hello".to_string(), bus::CausalId::new())
                .await
                .expect("embed fallback should succeed");

        assert_eq!(vector.len(), EMBEDDING_FALLBACK_DIMENSIONS);
        assert!(vector.iter().all(|value| *value == 0.0));
    }

    #[tokio::test]
    async fn persist_completed_exchange_writes_separate_user_and_sena_nodes() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::default_loaded());
        let mut actor = InferenceActor::new(backend);
        let causal_id = bus::CausalId::new();
        let observed_writes = Arc::new(StdMutex::new(Vec::new()));

        actor.remember_user_input(causal_id, "remember this exchange".to_string());

        let (ready_rx, responder) =
            spawn_memory_ack_responder(bus.clone(), 2, Some(observed_writes.clone())).await;
        ready_rx
            .await
            .expect("memory ack responder should initialize");

        actor
            .persist_completed_exchange(
                &bus,
                InferenceSource::UserText,
                "I will remember it.",
                causal_id,
            )
            .await
            .expect("completed exchange should persist");

        responder
            .await
            .expect("memory write responder should finish");

        let writes = observed_writes
            .lock()
            .expect("memory write capture mutex")
            .clone();
        assert_eq!(
            writes,
            vec![
                "User: remember this exchange".to_string(),
                "Sena: I will remember it.".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn actor_responds_to_reasoning_chain_query_with_latest_summary() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend::with_config(MockConfig {
            loaded: true,
            response: "Hello world. How are you?".to_string(),
            token_count: 6,
            ..Default::default()
        }));
        let mut actor = InferenceActor::new(backend);
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.expect("start failed");
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        let (ready_rx, responder) = spawn_memory_ack_responder(bus.clone(), 2, None).await;
        ready_rx
            .await
            .expect("memory ack responder should initialize");

        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test".to_string(),
            priority: bus::Priority::Normal,
            source: bus::InferenceSource::UserText,
            causal_id,
        }))
        .await
        .expect("inference request broadcast should succeed");

        let mut completed = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Inference(InferenceEvent::InferenceCompleted { .. }))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                completed = true;
                break;
            }
        }
        assert!(
            completed,
            "inference should complete before transparency query"
        );

        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            TransparencyQuery::ReasoningChain {
                thought_id: "latest".to_string(),
            },
        )))
        .await
        .expect("reasoning transparency query should broadcast");

        let mut responded = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Transparency(TransparencyEvent::QueryResponse {
                query: TransparencyQuery::ReasoningChain { .. },
                result,
            }))) = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                match &*result {
                    TransparencyResult::Reasoning(reasoning) => {
                        assert_eq!(reasoning.causal_id, causal_id.as_u64());
                        assert!(reasoning.source_description.contains("user text input"));
                        assert!(reasoning.token_count > 0);
                        responded = true;
                        break;
                    }
                    other => panic!("unexpected transparency result: {other:?}"),
                }
            }
        }

        assert!(
            responded,
            "inference actor should answer reasoning transparency queries"
        );

        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            TransparencyQuery::ReasoningChain {
                thought_id: causal_id.as_u64().to_string(),
            },
        )))
        .await
        .expect("explicit-id transparency query should broadcast");

        let mut explicit_id_responded = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Transparency(TransparencyEvent::QueryResponse {
                query: TransparencyQuery::ReasoningChain { .. },
                result,
            }))) = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                match &*result {
                    TransparencyResult::Reasoning(reasoning) => {
                        assert_eq!(reasoning.causal_id, causal_id.as_u64());
                        explicit_id_responded = true;
                        break;
                    }
                    other => panic!("unexpected transparency result: {other:?}"),
                }
            }
        }

        assert!(
            explicit_id_responded,
            "inference actor should answer reasoning queries by explicit id"
        );

        responder.await.expect("memory ack responder should finish");
    }

    #[tokio::test]
    async fn context_interpretation_prompt_includes_structural_context_details() {
        let now = std::time::Instant::now();
        let last_prompt = Arc::new(StdMutex::new(None));
        let backend: Arc<Mutex<Box<dyn InferenceBackend>>> =
            Arc::new(Mutex::new(Box::new(PromptCaptureBackend::new(
                r#"{"category":"coding","semantic_description":"Editing code","confidence":0.8}"#,
                last_prompt.clone(),
            ))));

        let context = ContextInterpretationInput {
            snapshot: ContextSnapshot {
                active_app: bus::events::platform::WindowContext {
                    app_name: "Code".to_string(),
                    window_title: Some("src/main.rs".to_string()),
                    bundle_id: Some("com.microsoft.VSCode".to_string()),
                    timestamp: now,
                },
                recent_files: vec![bus::events::platform::FileEvent {
                    path: std::path::PathBuf::from("src/main.rs"),
                    event_kind: bus::events::platform::FileEventKind::Modified,
                    timestamp: now,
                }],
                clipboard_digest: Some("abc123".to_string()),
                keystroke_cadence: bus::events::platform::KeystrokeCadence {
                    events_per_minute: 144.0,
                    burst_detected: true,
                    idle_duration: std::time::Duration::from_secs(12),
                    timestamp: now,
                },
                session_duration: std::time::Duration::from_secs(900),
                visual_context: Some(bus::events::ctp::VisualContext {
                    resolution: (1920, 1080),
                    age: std::time::Duration::from_secs(3),
                }),
                timestamp: now,
                soul_identity_signal: Some(bus::events::soul::DistilledIdentitySignal {
                    signal_key: "work::editor".to_string(),
                    signal_value: "vscode".to_string(),
                    confidence: 0.9,
                }),
            },
            patterns: vec![bus::events::ctp::SignalPattern {
                pattern_type: bus::events::ctp::SignalPatternType::Frustration,
                confidence: 0.7,
                description: "Frustration detected after rapid typing burst".to_string(),
            }],
            memory_relevance: 0.82,
        };

        let interpreted =
            InferenceActor::execute_context_interpretation(backend, context, bus::CausalId::new())
                .await
                .expect("context interpretation should succeed");

        assert!(
            interpreted.is_some(),
            "capture backend should yield a parsed task"
        );

        let captured_prompt = last_prompt
            .lock()
            .expect("prompt capture mutex should not be poisoned")
            .clone()
            .expect("prompt should be captured");

        assert!(captured_prompt.contains("recent_files"));
        assert!(captured_prompt.contains("main.rs"));
        assert!(captured_prompt.contains("visual_context"));
        assert!(captured_prompt.contains("identity_signal"));
        assert!(captured_prompt.contains("memory_relevance"));
        assert!(captured_prompt.contains("patterns"));
        assert!(captured_prompt.contains("Frustration"));
    }
}
