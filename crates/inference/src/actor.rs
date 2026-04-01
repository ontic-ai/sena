//! Inference actor: model discovery, lazy loading, and inference execution.

use async_trait::async_trait;
use bus::events::ctp::{CTPEvent, ContextSnapshot};
use bus::events::inference::Priority;
use bus::events::memory::{MemoryChunk, MemoryQueryRequest, MemoryWriteRequest};
use bus::events::transparency::{TransparencyEvent, TransparencyQuery};
use bus::events::InferenceEvent;
use bus::{Actor, ActorError, Event, EventBus, MemoryEvent, SystemEvent};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

use crate::backend::{BackendType, InferenceParams, LlmBackend};
use crate::chat_template::ChatTemplate;
use crate::discovery;
use crate::queue::{InferenceQueue, QueuedWork, WorkKind};
use crate::registry::ModelRegistry;
use crate::transparency_query::{handle_transparency_query, InferenceState};

/// Default inference queue capacity.
const DEFAULT_QUEUE_CAPACITY: usize = 128;

/// Default directed channel capacity.
const DEFAULT_DIRECTED_CAPACITY: usize = 64;
const ITERATIVE_MAX_HARD_CAP: usize = 6;
const MEMORY_QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const MEMORY_QUERY_TOKEN_BUDGET: usize = 8;
/// Timeout for single-round memory queries. 2 seconds allows memory to embed the query
/// and return results. We process directed events during the wait to avoid deadlock.
const SINGLE_ROUND_MEMORY_TIMEOUT: Duration = Duration::from_secs(2);

/// Inference actor manages model discovery, lazy loading, and inference requests.
///
/// The actor:
/// 1. Discovers models at start (from Ollama manifest)
/// 2. Lazily loads model weights on first request
/// 3. Processes inference/embed/extract requests from a priority queue
/// 4. All backend calls run in spawn_blocking
pub struct InferenceActor {
    registry: Option<ModelRegistry>,
    models_dir: PathBuf,
    /// Preferred model name from config; overrides auto-selected largest model.
    preferred_model: Option<String>,
    /// Name of the currently loaded model (set when model is loaded).
    current_model_name: Option<String>,
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    directed_rx: Option<mpsc::Receiver<Event>>,
    backend: Arc<Mutex<Box<dyn LlmBackend>>>,
    backend_type: BackendType,
    queue: InferenceQueue,
    /// Captures state from the most recent successful inference cycle.
    last_inference_state: Option<InferenceState>,
    /// Latest CTP context snapshot.
    last_snapshot: Option<ContextSnapshot>,
    /// Recent conversation history (user, assistant pairs), capped at 5.
    conversation_history: Vec<(String, String)>,
}

impl InferenceActor {
    /// Create a new inference actor with auto-detected backend and models directory.
    ///
    /// The backend is automatically selected based on platform and available hardware:
    /// - macOS → Metal
    /// - Windows/Linux with NVIDIA GPU → CUDA
    /// - Otherwise → CPU
    pub fn new(models_dir: PathBuf, backend: Box<dyn LlmBackend>) -> Self {
        let backend_type = BackendType::auto_detect();
        Self {
            registry: None,
            models_dir,
            preferred_model: None,
            current_model_name: None,
            bus: None,
            bus_rx: None,
            directed_rx: None,
            backend: Arc::new(Mutex::new(backend)),
            backend_type,
            queue: InferenceQueue::new(DEFAULT_QUEUE_CAPACITY),
            last_inference_state: None,
            last_snapshot: None,
            conversation_history: Vec::new(),
        }
    }

    /// Override the default model with a user-preferred model name.
    ///
    /// If the preferred model is found in the registry after discovery,
    /// it will be used for inference instead of the largest auto-selected model.
    pub fn with_preferred_model(mut self, preferred: Option<String>) -> Self {
        self.preferred_model = preferred;
        self
    }

    /// Returns a reference to the model registry, if discovery succeeded.
    pub fn registry(&self) -> Option<&ModelRegistry> {
        self.registry.as_ref()
    }

    /// Ensure the model is loaded. On first call, loads lazily via spawn_blocking.
    async fn ensure_loaded(&mut self, bus: &Arc<EventBus>) -> Result<(), String> {
        {
            let guard = self
                .backend
                .lock()
                .map_err(|e| format!("backend lock poisoned: {}", e))?;
            if guard.is_loaded() {
                return Ok(());
            }
        }

        let model_path = self
            .registry
            .as_ref()
            .and_then(|r| {
                r.default_model()
                    .and_then(|name| r.find_by_name(name))
                    .map(|m| m.path.clone())
            })
            .ok_or_else(|| "no model available for loading".to_string())?;

        let model_name = self
            .registry
            .as_ref()
            .and_then(|r| r.default_model().map(String::from))
            .unwrap_or_default();

        let backend_type = self.backend_type;
        let backend_clone = self.backend.clone();
        let path_clone = model_path;

        let load_result = tokio::task::spawn_blocking(move || {
            let mut guard = backend_clone
                .lock()
                .map_err(|e| format!("backend lock poisoned: {}", e))?;
            guard
                .load_model(&path_clone, backend_type)
                .map_err(|e| format!("{}", e))
        })
        .await
        .map_err(|e| format!("load task panicked: {}", e))?;

        match load_result {
            Ok(()) => {
                // Set current model name for chat template detection
                self.current_model_name = Some(model_name.clone());

                let _ = bus
                    .broadcast(Event::Inference(InferenceEvent::ModelLoaded {
                        name: model_name,
                        backend: self.backend_type.to_string(),
                    }))
                    .await;
                Ok(())
            }
            Err(e) => Err(format!("model load failed: {}", e)),
        }
    }

    /// Process a single queued work item.
    async fn process_work(&mut self, work: QueuedWork, bus: &Arc<EventBus>) {
        if let Err(err) = self.ensure_loaded(bus).await {
            match work.kind {
                WorkKind::Infer { response_tx, .. } => {
                    let _ = response_tx.send(Err(err.clone()));
                }
                WorkKind::Embed { response_tx, .. } => {
                    let _ = response_tx.send(Err(err.clone()));
                }
                WorkKind::Extract { response_tx, .. } => {
                    let _ = response_tx.send(Err(err.clone()));
                }
            }
            let _ = bus
                .broadcast(Event::Inference(InferenceEvent::InferenceFailed {
                    request_id: work.request_id,
                    reason: err,
                }))
                .await;
            return;
        }

        let backend = self.backend.clone();
        let request_id = work.request_id;

        match work.kind {
            WorkKind::Infer {
                prompt,
                response_tx,
            } => {
                // Detect chat template and wrap prompt
                let template = self
                    .current_model_name
                    .as_ref()
                    .map(|name| ChatTemplate::detect_from_model_name(name))
                    .unwrap_or(ChatTemplate::Raw);
                let wrapped_prompt = template.wrap(&prompt);

                let backend_clone = backend;
                let prompt_len = prompt.len();
                let result = tokio::task::spawn_blocking(move || {
                    let guard = backend_clone
                        .lock()
                        .map_err(|e| format!("lock poisoned: {}", e))?;
                    guard
                        .infer(&wrapped_prompt, &InferenceParams::default())
                        .map_err(|e| format!("{}", e))
                })
                .await;

                match result {
                    Ok(Ok(text)) => {
                        let token_count = text.split_whitespace().count();
                        let _ = response_tx.send(Ok((text.clone(), token_count)));

                        // Capture inference state for transparency queries
                        self.last_inference_state = Some(InferenceState {
                            request_context: format!("Inference request: {} chars", prompt_len),
                            response_text: text.clone(),
                            working_memory_context: vec![],
                            rounds_completed: 1,
                        });

                        let _ = bus
                            .broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
                                text,
                                request_id,
                                token_count,
                            }))
                            .await;
                    }
                    Ok(Err(e)) => {
                        let _ = response_tx.send(Err(e.clone()));
                        let _ = bus
                            .broadcast(Event::Inference(InferenceEvent::InferenceFailed {
                                request_id,
                                reason: e,
                            }))
                            .await;
                    }
                    Err(e) => {
                        let err = format!("task panicked: {}", e);
                        let _ = response_tx.send(Err(err.clone()));
                        let _ = bus
                            .broadcast(Event::Inference(InferenceEvent::InferenceFailed {
                                request_id,
                                reason: err,
                            }))
                            .await;
                    }
                }
            }
            WorkKind::Embed { text, response_tx } => {
                let backend_clone = backend;
                let result = tokio::task::spawn_blocking(move || {
                    let guard = backend_clone
                        .lock()
                        .map_err(|e| format!("lock poisoned: {}", e))?;
                    guard.embed(&text).map_err(|e| format!("{}", e))
                })
                .await;

                match result {
                    Ok(Ok(vector)) => {
                        let _ = response_tx.send(Ok(vector.clone()));
                        let _ = bus
                            .broadcast(Event::Inference(InferenceEvent::EmbedCompleted {
                                vector,
                                request_id,
                            }))
                            .await;
                    }
                    Ok(Err(e)) => {
                        let _ = response_tx.send(Err(e));
                    }
                    Err(e) => {
                        let _ = response_tx.send(Err(format!("task panicked: {}", e)));
                    }
                }
            }
            WorkKind::Extract { text, response_tx } => {
                let backend_clone = backend;
                let result = tokio::task::spawn_blocking(move || {
                    let guard = backend_clone
                        .lock()
                        .map_err(|e| format!("lock poisoned: {}", e))?;
                    guard.extract(&text).map_err(|e| format!("{}", e))
                })
                .await;

                match result {
                    Ok(Ok(facts)) => {
                        let _ = response_tx.send(Ok(facts.clone()));
                        let _ = bus
                            .broadcast(Event::Inference(InferenceEvent::ExtractionCompleted {
                                facts,
                                request_id,
                            }))
                            .await;
                    }
                    Ok(Err(e)) => {
                        let _ = response_tx.send(Err(e));
                    }
                    Err(e) => {
                        let _ = response_tx.send(Err(format!("task panicked: {}", e)));
                    }
                }
            }
        }
    }

    async fn infer_once(&mut self, prompt: String, bus: &Arc<EventBus>) -> Result<String, String> {
        self.ensure_loaded(bus).await?;

        // Detect chat template and wrap prompt
        let template = self
            .current_model_name
            .as_ref()
            .map(|name| ChatTemplate::detect_from_model_name(name))
            .unwrap_or(ChatTemplate::Raw);
        let wrapped_prompt = template.wrap(&prompt);

        let backend_clone = self.backend.clone();
        let result = tokio::task::spawn_blocking(move || {
            let guard = backend_clone
                .lock()
                .map_err(|e| format!("lock poisoned: {}", e))?;
            guard
                .infer(&wrapped_prompt, &InferenceParams::default())
                .map_err(|e| format!("{}", e))
        })
        .await
        .map_err(|e| format!("task panicked: {}", e))?;

        result
    }

    /// Build enriched prompt from user message, memory chunks, context snapshot, and history.
    ///
    /// The prompt includes:
    /// 1. System instruction coaching the LLM to use context, memory, and history
    /// 2. Relevant memory chunks from the ech0 store (if any)
    /// 3. Recent conversation history to maintain continuity (if any)
    /// 4. Current computing context from CTP (if available)
    /// 5. The user's current message
    ///
    /// All sections are omitted if empty, ensuring compact prompts for new conversations.
    fn build_enriched_prompt(
        user_message: &str,
        memory_chunks: &[MemoryChunk],
        snapshot: Option<&ContextSnapshot>,
        history: &[(String, String)],
    ) -> String {
        let mut parts = Vec::new();

        // System instruction: tell the LLM it has memory and context available
        let has_memory = !memory_chunks.is_empty();
        let has_history = !history.is_empty();
        let has_context = snapshot.is_some();

        let mut instruction = String::from(
            "You are Sena, an AI assistant with access to persistent memory and context.",
        );
        if has_memory {
            instruction
                .push_str(" Review the provided memory to understand relevant prior knowledge.");
        }
        if has_history {
            instruction.push_str(" Use the recent conversation history to maintain continuity and remember what you've already discussed.");
        }
        if has_context {
            instruction.push_str(" Consider the current context (active application, inferred task, keystroke activity) when providing assistance.");
        }
        instruction.push_str(" When the user asks about past interactions or what you know, draw on these sources of information.");

        parts.push(format!("## System\n{}", instruction));

        // Add relevant memory if any
        if !memory_chunks.is_empty() {
            let lines: Vec<String> = memory_chunks
                .iter()
                .map(|c| format!("- {}", c.text))
                .collect();
            parts.push(format!("## Relevant Memory\n{}", lines.join("\n")));
        }

        // Add recent conversation history to maintain continuity
        if !history.is_empty() {
            let hist_lines: Vec<String> = history
                .iter()
                .map(|(u, a)| format!("User: {}\nAssistant: {}", u, a))
                .collect();
            parts.push(format!(
                "## Recent Conversation\n{}",
                hist_lines.join("\n\n")
            ));
        }

        // Add active context if available
        if let Some(snap) = snapshot {
            let mut ctx_parts = vec![format!("Active application: {}", snap.active_app.app_name)];
            if let Some(title) = &snap.active_app.window_title {
                ctx_parts.push(format!("Window: {}", title));
            }
            if let Some(task) = &snap.inferred_task {
                ctx_parts.push(format!(
                    "Inferred task: {} ({:.0}%)",
                    task.category,
                    task.confidence * 100.0
                ));
            }
            if snap.keystroke_cadence.events_per_minute > 0.0 {
                ctx_parts.push(format!(
                    "Typing activity: {:.1} events/min",
                    snap.keystroke_cadence.events_per_minute
                ));
            }
            parts.push(format!("## Current Context\n{}", ctx_parts.join("\n")));
        }

        // Add the current user message last
        parts.push(format!("## User\n{}", user_message));

        parts.join("\n\n")
    }

    /// Process a single inference request with memory context enrichment.
    /// This runs directly in the event loop (not queued) so the actor can handle
    /// Embed/Extract requests from memory while waiting for query responses.
    /// Uses a short timeout for memory queries to avoid blocking.
    async fn process_single_inference_with_context(
        &mut self,
        bus: &Arc<EventBus>,
        prompt: String,
        request_id: u64,
    ) -> Result<(String, usize), String> {
        self.ensure_loaded(bus).await?;

        // Query memory for relevant context
        let memory_request_id = request_id.saturating_mul(1000);
        let memory_chunks = self
            .query_memory_with_timeout(bus, &prompt, memory_request_id, SINGLE_ROUND_MEMORY_TIMEOUT)
            .await
            .unwrap_or_else(|_| Vec::new());

        // Build enriched prompt with context, memory, and conversation history
        let enriched_prompt = Self::build_enriched_prompt(
            &prompt,
            &memory_chunks,
            self.last_snapshot.as_ref(),
            &self.conversation_history,
        );

        // Detect chat template and wrap enriched prompt
        let template = self
            .current_model_name
            .as_ref()
            .map(|name| ChatTemplate::detect_from_model_name(name))
            .unwrap_or(ChatTemplate::Raw);
        let wrapped_prompt = template.wrap(&enriched_prompt);

        let backend_clone = self.backend.clone();
        let prompt_len = enriched_prompt.len();
        let result = tokio::task::spawn_blocking(move || {
            let guard = backend_clone
                .lock()
                .map_err(|e| format!("lock poisoned: {}", e))?;
            guard
                .infer(&wrapped_prompt, &InferenceParams::default())
                .map_err(|e| format!("{}", e))
        })
        .await
        .map_err(|e| format!("task panicked: {}", e))??;

        // Update conversation history
        self.conversation_history
            .push((prompt.clone(), result.clone()));
        if self.conversation_history.len() > 5 {
            self.conversation_history.remove(0);
        }

        // Write conversation to memory (non-fatal if fails)
        let memory_write_request_id = request_id.saturating_mul(2000);
        let conversation_text = format!("User: {}\nAssistant: {}", prompt, result);
        let _ = bus
            .send_directed(
                "memory",
                Event::Memory(MemoryEvent::WriteRequested(MemoryWriteRequest {
                    text: conversation_text,
                    request_id: memory_write_request_id,
                })),
            )
            .await;

        // Capture inference state for transparency queries
        let token_count = result.split_whitespace().count();
        self.last_inference_state = Some(InferenceState {
            request_context: format!("Inference request: {} chars", prompt_len),
            response_text: result.clone(),
            working_memory_context: memory_chunks,
            rounds_completed: 1,
        });

        Ok((result, token_count))
    }

    async fn query_memory_for_round(
        &mut self,
        bus: &Arc<EventBus>,
        query: &str,
        request_id: u64,
    ) -> Result<Vec<MemoryChunk>, String> {
        self.query_memory_with_timeout(bus, query, request_id, MEMORY_QUERY_TIMEOUT)
            .await
    }

    async fn query_memory_with_timeout(
        &mut self,
        bus: &Arc<EventBus>,
        query: &str,
        request_id: u64,
        timeout: Duration,
    ) -> Result<Vec<MemoryChunk>, String> {
        let mut rx = bus.subscribe_broadcast();

        bus.send_directed(
            "memory",
            Event::Memory(MemoryEvent::QueryRequested(MemoryQueryRequest {
                query: query.to_owned(),
                token_budget: MEMORY_QUERY_TOKEN_BUDGET,
                request_id,
            })),
        )
        .await
        .map_err(|e| format!("memory query dispatch failed: {}", e))?;

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(Vec::new());
            }

            tokio::select! {
                // Check for memory query response
                event = rx.recv() => {
                    match event {
                        Ok(Event::Memory(MemoryEvent::QueryCompleted(resp)))
                            if resp.request_id == request_id =>
                        {
                            return Ok(resp.chunks);
                        }
                        Ok(_) => continue,
                        Err(_) => return Ok(Vec::new()),
                    }
                }
                // Process directed events (Embed/Extract requests from memory) to avoid deadlock
                directed = async {
                    match &mut self.directed_rx {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                } => {
                    if let Some(event) = directed {
                        match event {
                            Event::Inference(InferenceEvent::EmbedRequested { text, request_id }) => {
                                let (tx, _rx) = tokio::sync::oneshot::channel();
                                let _ = self.queue.enqueue(
                                    Priority::Normal,
                                    request_id,
                                    WorkKind::Embed { text, response_tx: tx },
                                );
                                // Process the queued work immediately to avoid blocking memory
                                if let Some(work) = self.queue.dequeue() {
                                    self.process_work(work, bus).await;
                                }
                            }
                            Event::Inference(InferenceEvent::ExtractionRequested { text, request_id }) => {
                                let (tx, _rx) = tokio::sync::oneshot::channel();
                                let _ = self.queue.enqueue(
                                    Priority::Normal,
                                    request_id,
                                    WorkKind::Extract { text, response_tx: tx },
                                );
                                // Process the queued work immediately to avoid blocking memory
                                if let Some(work) = self.queue.dequeue() {
                                    self.process_work(work, bus).await;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    async fn process_iterative_request(
        &mut self,
        bus: &Arc<EventBus>,
        prompt: String,
        request_id: u64,
        max_rounds: usize,
    ) -> Result<(String, usize), String> {
        let rounds = max_rounds.clamp(1, ITERATIVE_MAX_HARD_CAP);
        let mut composed_prompt = prompt.clone();
        let mut final_text = String::new();
        let mut working_memory: Vec<MemoryChunk> = Vec::new();
        let mut actual_rounds_completed = 1;

        for round in 1..=rounds {
            let text = self.infer_once(composed_prompt.clone(), bus).await?;
            final_text = text.clone();
            actual_rounds_completed = round;

            let _ = bus
                .broadcast(Event::Inference(InferenceEvent::InferenceRoundCompleted {
                    text: text.clone(),
                    request_id,
                    round,
                    total_rounds: rounds,
                }))
                .await;

            if round == rounds {
                break;
            }

            let memory_request_id = request_id.saturating_mul(100).saturating_add(round as u64);
            let memory_chunks = self
                .query_memory_for_round(bus, &text, memory_request_id)
                .await?;
            if memory_chunks.is_empty() {
                break;
            }

            // Capture working memory for transparency query
            working_memory = memory_chunks.clone();

            // Extract text from chunks for prompt composition
            let memory_text: Vec<String> = memory_chunks.into_iter().map(|c| c.text).collect();
            composed_prompt = format!(
                "{}\n\n{}\n\n{}",
                composed_prompt,
                text,
                memory_text.join("\n")
            );
        }

        let token_count = final_text.split_whitespace().count();

        // Capture inference state for transparency queries
        self.last_inference_state = Some(InferenceState {
            request_context: format!(
                "Iterative inference: {} rounds, {} chars",
                actual_rounds_completed,
                prompt.len()
            ),
            response_text: final_text.clone(),
            working_memory_context: working_memory,
            rounds_completed: actual_rounds_completed,
        });

        Ok((final_text, token_count))
    }
}

#[async_trait]
impl Actor for InferenceActor {
    fn name(&self) -> &'static str {
        "inference"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        self.bus_rx = Some(bus.subscribe_broadcast());

        // Register directed channel
        let (tx, rx) = mpsc::channel(DEFAULT_DIRECTED_CAPACITY);
        bus.register_directed("inference", tx)
            .map_err(|e| ActorError::StartupFailed(format!("register directed failed: {}", e)))?;
        self.directed_rx = Some(rx);

        // Check for backend mismatch: GPU detected but CPU-only llama-cpp-2 build
        // Currently compiled without GPU features. When GPU features are added,
        // update this check to: cfg!(feature = "cuda") || cfg!(feature = "metal")
        let gpu_features_compiled = false;
        if !gpu_features_compiled
            && (self.backend_type == BackendType::Cuda || self.backend_type == BackendType::Metal)
        {
            let warning_event = InferenceEvent::BackendMismatchWarning {
                detected: format!("{}", self.backend_type),
                compiled: "CPU-only".to_string(),
            };
            bus.broadcast(Event::Inference(warning_event))
                .await
                .map_err(|e| {
                    ActorError::StartupFailed(format!("broadcast warning failed: {}", e))
                })?;
        }

        // Perform model discovery
        match discovery::discover_models(&self.models_dir) {
            Ok(mut registry) => {
                // Apply user-preferred model override if set and found in registry.
                if let Some(preferred) = &self.preferred_model {
                    registry.set_preferred_model(preferred);
                }
                let event = InferenceEvent::ModelRegistryBuilt {
                    model_count: registry.model_count(),
                    default_model: registry.default_model().map(String::from),
                };
                bus.broadcast(Event::Inference(event))
                    .await
                    .map_err(|e| ActorError::StartupFailed(format!("broadcast failed: {}", e)))?;
                self.registry = Some(registry);
            }
            Err(e) => {
                let event = InferenceEvent::ModelDiscoveryFailed {
                    reason: e.to_string(),
                };
                bus.broadcast(Event::Inference(event))
                    .await
                    .map_err(|e| ActorError::StartupFailed(format!("broadcast failed: {}", e)))?;
            }
        }

        self.bus = Some(bus.clone());

        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: "Inference",
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e)))?;

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let bus = self
            .bus
            .clone()
            .ok_or_else(|| ActorError::RuntimeError("bus not initialized".to_string()))?;

        loop {
            // Process any queued work first
            while let Some(work) = self.queue.dequeue() {
                self.process_work(work, &bus).await;
            }

            tokio::select! {
                event = async {
                    match &mut self.bus_rx {
                        Some(rx) => rx.recv().await,
                        None => Err(broadcast::error::RecvError::Closed),
                    }
                } => {
                    match event {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                            return Ok(());
                        }
                        Ok(Event::CTP(CTPEvent::ContextSnapshotReady(snapshot))) => {
                            self.last_snapshot = Some(snapshot);
                        }
                        Ok(Event::Transparency(TransparencyEvent::QueryRequested(
                            TransparencyQuery::InferenceExplanation,
                        ))) => {
                            let state = self.last_inference_state.clone();
                            let b = bus.clone();
                            tokio::spawn(async move {
                                let response = handle_transparency_query(&state).await;
                                let _ = b
                                    .broadcast(Event::Transparency(
                                        TransparencyEvent::InferenceExplanationResponded(response),
                                    ))
                                    .await;
                            });
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(ActorError::ChannelClosed("bus channel closed".to_string()));
                        }
                        _ => {}
                    }
                }
                directed = async {
                    match &mut self.directed_rx {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                } => {
                    if let Some(event) = directed {
                        match event {
                            Event::Inference(InferenceEvent::InferenceRequested { prompt, priority: _, request_id }) => {
                                // Process with memory context directly in event handler with short timeout
                                // to avoid blocking on memory queries that require embeddings
                                match self
                                    .process_single_inference_with_context(&bus, prompt, request_id)
                                    .await
                                {
                                    Ok((text, token_count)) => {
                                        let _ = bus
                                            .broadcast(Event::Inference(
                                                InferenceEvent::InferenceCompleted {
                                                    text,
                                                    request_id,
                                                    token_count,
                                                },
                                            ))
                                            .await;
                                    }
                                    Err(reason) => {
                                        let _ = bus
                                            .broadcast(Event::Inference(
                                                InferenceEvent::InferenceFailed {
                                                    request_id,
                                                    reason,
                                                },
                                            ))
                                            .await;
                                    }
                                }
                            }
                            Event::Inference(InferenceEvent::EmbedRequested { text, request_id }) => {
                                let (tx, _rx) = tokio::sync::oneshot::channel();
                                let _ = self.queue.enqueue(
                                    Priority::Normal,
                                    request_id,
                                    WorkKind::Embed { text, response_tx: tx },
                                );
                            }
                            Event::Inference(InferenceEvent::ExtractionRequested { text, request_id }) => {
                                let (tx, _rx) = tokio::sync::oneshot::channel();
                                let _ = self.queue.enqueue(
                                    Priority::Normal,
                                    request_id,
                                    WorkKind::Extract { text, response_tx: tx },
                                );
                            }
                            Event::Inference(InferenceEvent::InferenceRequestedIterative {
                                prompt,
                                priority: _,
                                request_id,
                                max_rounds,
                            }) => {
                                match self
                                    .process_iterative_request(&bus, prompt, request_id, max_rounds)
                                    .await
                                {
                                    Ok((text, token_count)) => {
                                        let _ = bus
                                            .broadcast(Event::Inference(
                                                InferenceEvent::InferenceCompleted {
                                                    text,
                                                    request_id,
                                                    token_count,
                                                },
                                            ))
                                            .await;
                                    }
                                    Err(reason) => {
                                        let _ = bus
                                            .broadcast(Event::Inference(
                                                InferenceEvent::InferenceFailed {
                                                    request_id,
                                                    reason,
                                                },
                                            ))
                                            .await;
                                    }
                                }
                            }
                            _ => {}
                        }
                    } else {
                        return Err(ActorError::ChannelClosed("directed channel closed".to_string()));
                    }
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        self.bus_rx = None;
        self.directed_rx = None;
        self.bus = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_backend::MockBackend;
    use std::fs;
    use std::time::{Duration, Instant, SystemTime};
    use tempfile::tempdir;

    fn create_mock_ollama_structure(base_dir: &std::path::Path) {
        let manifests_lib = base_dir
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library");
        fs::create_dir_all(&manifests_lib).expect("create manifests dir");

        let model_dir = manifests_lib.join("test-model");
        fs::create_dir_all(&model_dir).expect("create model dir");

        let manifest_json = r#"{
  "schemaVersion": 2,
  "layers": [
    {
      "mediaType": "application/vnd.ollama.image.model",
      "digest": "sha256:testdigest123",
      "size": 3000000000
    }
  ]
}"#;
        fs::write(model_dir.join("latest"), manifest_json).expect("write manifest");

        let blobs_dir = base_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).expect("create blobs dir");
        fs::write(blobs_dir.join("sha256-testdigest123"), vec![0u8; 1024]).expect("write blob");
    }

    fn mock_backend() -> Box<dyn LlmBackend> {
        Box::new(MockBackend::new())
    }

    fn mock_actor(models_dir: PathBuf) -> InferenceActor {
        InferenceActor::new(models_dir, mock_backend())
    }

    #[test]
    fn inference_actor_implements_actor_trait() {
        let actor = mock_actor(PathBuf::from("/tmp/test"));
        assert_eq!(actor.name(), "inference");
    }

    #[tokio::test]
    async fn inference_actor_discovers_models_on_start() {
        let temp_dir = tempdir().expect("create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let mut actor = mock_actor(temp_dir.path().to_path_buf());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus).await.expect("start should succeed");

        assert!(actor.registry().is_some());
        assert_eq!(actor.registry().map(|r| r.model_count()), Some(1));

        // May emit BackendMismatchWarning first if GPU detected but not compiled
        let mut event = rx.recv().await.expect("should receive event");
        if matches!(
            event,
            Event::Inference(InferenceEvent::BackendMismatchWarning { .. })
        ) {
            event = rx.recv().await.expect("should receive second event");
        }

        match event {
            Event::Inference(InferenceEvent::ModelRegistryBuilt {
                model_count,
                default_model,
            }) => {
                assert_eq!(model_count, 1);
                assert!(default_model.is_some());
            }
            other => panic!("Expected ModelRegistryBuilt, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn inference_actor_emits_failure_when_no_models() {
        let temp_dir = tempdir().expect("create temp dir");

        let mut actor = mock_actor(temp_dir.path().join("nonexistent"));
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus)
            .await
            .expect("start should succeed even with no models");

        assert!(actor.registry().is_none());

        // May emit BackendMismatchWarning first if GPU detected but not compiled
        let mut event = rx.recv().await.expect("should receive event");
        if matches!(
            event,
            Event::Inference(InferenceEvent::BackendMismatchWarning { .. })
        ) {
            event = rx.recv().await.expect("should receive second event");
        }

        match event {
            Event::Inference(InferenceEvent::ModelDiscoveryFailed { reason }) => {
                assert!(!reason.is_empty());
            }
            other => panic!("Expected ModelDiscoveryFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn inference_actor_stops_on_shutdown_signal() {
        let temp_dir = tempdir().expect("create temp dir");
        let mut actor = mock_actor(temp_dir.path().join("nonexistent"));

        let bus = Arc::new(EventBus::new());
        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        let run_handle = tokio::spawn(async move { actor.run().await });

        tokio::time::sleep(Duration::from_millis(50)).await;

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        let result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");
        assert!(result.expect("join handle").is_ok(), "run should return Ok");
    }

    #[tokio::test]
    async fn inference_actor_starts_and_stops() {
        let temp_dir = tempdir().expect("create temp dir");
        let mut actor = mock_actor(temp_dir.path().to_path_buf());

        let bus = Arc::new(EventBus::new());
        actor.start(bus).await.expect("start should succeed");

        actor.stop().await.expect("stop should succeed");
        assert!(actor.bus_rx.is_none());
        assert!(actor.bus.is_none());
    }

    #[tokio::test]
    async fn inference_actor_processes_directed_inference_request() {
        let temp_dir = tempdir().expect("create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let mut actor = mock_actor(temp_dir.path().to_path_buf());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Drain the ModelRegistryBuilt event
        let _ = rx.recv().await.expect("should get registry event");

        // Send directed inference request
        bus.send_directed(
            "inference",
            Event::Inference(InferenceEvent::InferenceRequested {
                prompt: "test prompt".to_string(),
                priority: Priority::Normal,
                request_id: 42,
            }),
        )
        .await
        .expect("send directed should succeed");

        // Run actor briefly to process the request
        let run_handle = tokio::spawn(async move { actor.run().await });

        // Wait for processing
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Send shutdown
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        let result = tokio::time::timeout(Duration::from_secs(2), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");

        // Check for ModelLoaded and InferenceCompleted events
        let mut found_loaded = false;
        let mut found_completed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                Event::Inference(InferenceEvent::ModelLoaded { .. }) => {
                    found_loaded = true;
                }
                Event::Inference(InferenceEvent::InferenceCompleted { request_id, .. }) => {
                    assert_eq!(request_id, 42);
                    found_completed = true;
                }
                _ => {}
            }
        }
        assert!(found_loaded, "should emit ModelLoaded event");
        assert!(found_completed, "should emit InferenceCompleted event");
    }

    #[tokio::test]
    async fn inference_actor_processes_embed_request() {
        let temp_dir = tempdir().expect("create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let mut actor = mock_actor(temp_dir.path().to_path_buf());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");
        let _ = rx.recv().await.expect("should get registry event");

        bus.send_directed(
            "inference",
            Event::Inference(InferenceEvent::EmbedRequested {
                text: "test text".to_string(),
                request_id: 99,
            }),
        )
        .await
        .expect("send directed should succeed");

        let run_handle = tokio::spawn(async move { actor.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        let _ = tokio::time::timeout(Duration::from_secs(2), run_handle).await;

        let mut found_completed = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::Inference(InferenceEvent::EmbedCompleted { request_id, vector }) = event {
                assert_eq!(request_id, 99);
                assert_eq!(vector.len(), 384);
                found_completed = true;
            }
        }
        assert!(found_completed, "should emit EmbedCompleted event");
    }

    #[tokio::test]
    async fn inference_actor_processes_extract_request() {
        let temp_dir = tempdir().expect("create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let mut actor = mock_actor(temp_dir.path().to_path_buf());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");
        let _ = rx.recv().await.expect("should get registry event");

        bus.send_directed(
            "inference",
            Event::Inference(InferenceEvent::ExtractionRequested {
                text: "test text for extraction".to_string(),
                request_id: 77,
            }),
        )
        .await
        .expect("send directed should succeed");

        let run_handle = tokio::spawn(async move { actor.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        let _ = tokio::time::timeout(Duration::from_secs(2), run_handle).await;

        let mut found_completed = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::Inference(InferenceEvent::ExtractionCompleted { request_id, facts }) =
                event
            {
                assert_eq!(request_id, 77);
                assert_eq!(facts, vec!["fact1", "fact2"]);
                found_completed = true;
            }
        }
        assert!(found_completed, "should emit ExtractionCompleted event");
    }

    #[tokio::test]
    async fn inference_actor_captures_state_after_single_inference() {
        let temp_dir = tempdir().expect("create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let mut actor = mock_actor(temp_dir.path().to_path_buf());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");
        let _ = rx.recv().await; // drain ModelRegistryBuilt

        // Send a simple inference request
        bus.send_directed(
            "inference",
            Event::Inference(InferenceEvent::InferenceRequested {
                prompt: "test prompt".to_string(),
                priority: Priority::Normal,
                request_id: 123,
            }),
        )
        .await
        .expect("send directed should succeed");

        let run_handle = tokio::spawn(async move { actor.run().await });

        // Wait for processing
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Send shutdown
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        let result = tokio::time::timeout(Duration::from_secs(2), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");

        // Verify InferenceCompleted was broadcast
        let mut found_completed = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::Inference(InferenceEvent::InferenceCompleted {
                request_id, text, ..
            }) = event
            {
                assert_eq!(request_id, 123);
                // MockBackend returns "Mock inference response"
                assert_eq!(text, "Mock inference response");
                found_completed = true;
            }
        }
        assert!(found_completed, "should emit InferenceCompleted event");
    }

    #[tokio::test]
    async fn inference_actor_handles_transparency_query() {
        use bus::events::transparency::TransparencyQuery;

        let temp_dir = tempdir().expect("create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let mut actor = mock_actor(temp_dir.path().to_path_buf());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");
        let _ = rx.recv().await; // drain ModelRegistryBuilt

        // Send an inference request first
        bus.send_directed(
            "inference",
            Event::Inference(InferenceEvent::InferenceRequested {
                prompt: "what is rust?".to_string(),
                priority: Priority::Normal,
                request_id: 456,
            }),
        )
        .await
        .expect("send directed should succeed");

        let run_handle = tokio::spawn(async move { actor.run().await });

        // Wait for inference to complete
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Now query for transparency
        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            TransparencyQuery::InferenceExplanation,
        )))
        .await
        .expect("broadcast should succeed");

        // Wait a bit for processing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send shutdown
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        let result = tokio::time::timeout(Duration::from_secs(2), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");

        // Verify InferenceExplanationResponded was broadcast
        let mut found_response = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::Transparency(TransparencyEvent::InferenceExplanationResponded(resp)) =
                event
            {
                // Verify the response contains the inference data
                assert!(!resp.request_context.is_empty());
                assert_eq!(resp.response_text, "Mock inference response");
                assert_eq!(resp.rounds_completed, 1);
                found_response = true;
            }
        }
        assert!(
            found_response,
            "should emit InferenceExplanationResponded event"
        );
    }

    #[tokio::test]
    async fn inference_actor_handles_transparency_query_with_no_state() {
        use bus::events::transparency::TransparencyQuery;

        let temp_dir = tempdir().expect("create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let mut actor = mock_actor(temp_dir.path().to_path_buf());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");
        let _ = rx.recv().await; // drain ModelRegistryBuilt

        // Query for transparency WITHOUT running any inference first
        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            TransparencyQuery::InferenceExplanation,
        )))
        .await
        .expect("broadcast should succeed");

        let run_handle = tokio::spawn(async move { actor.run().await });

        // Wait for processing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send shutdown
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        let result = tokio::time::timeout(Duration::from_secs(2), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");

        // Verify InferenceExplanationResponded was broadcast with placeholder
        let mut found_response = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::Transparency(TransparencyEvent::InferenceExplanationResponded(resp)) =
                event
            {
                // Should have placeholder text since no inference ran
                assert_eq!(resp.request_context, "No inference cycle completed yet");
                assert_eq!(resp.response_text, "No inference cycle completed yet");
                assert_eq!(resp.rounds_completed, 0);
                assert!(resp.working_memory_context.is_empty());
                found_response = true;
            }
        }
        assert!(
            found_response,
            "should emit InferenceExplanationResponded event with placeholder"
        );
    }

    #[test]
    fn build_enriched_prompt_includes_system_instruction() {
        let prompt = InferenceActor::build_enriched_prompt("What do I remember?", &[], None, &[]);

        // Should include system instruction
        assert!(prompt.contains("## System"));
        assert!(prompt.contains("You are Sena"));
        assert!(prompt.contains("persistent memory"));
        assert!(prompt.contains("## User\nWhat do I remember?"));
    }

    #[test]
    fn build_enriched_prompt_includes_memory_chunks() {
        use bus::events::memory::MemoryChunk;

        let chunk1 = MemoryChunk {
            text: "You told me your favorite color is blue".into(),
            score: 0.95,
            timestamp: SystemTime::now(),
        };
        let chunk2 = MemoryChunk {
            text: "You work as a software engineer".into(),
            score: 0.87,
            timestamp: SystemTime::now(),
        };

        let prompt = InferenceActor::build_enriched_prompt(
            "Tell me about yourself",
            &[chunk1, chunk2],
            None,
            &[],
        );

        assert!(prompt.contains("## System"));
        assert!(prompt.contains("Review the provided memory"));
        assert!(prompt.contains("## Relevant Memory"));
        assert!(prompt.contains("You told me your favorite color is blue"));
        assert!(prompt.contains("You work as a software engineer"));
    }

    #[test]
    fn build_enriched_prompt_includes_conversation_history() {
        let history = vec![
            (
                "What is Rust?".into(),
                "Rust is a systems programming language.".into(),
            ),
            ("Is it fast?".into(), "Yes, Rust is very fast.".into()),
        ];

        let prompt = InferenceActor::build_enriched_prompt("Tell me more", &[], None, &history);

        assert!(prompt.contains("## System"));
        assert!(prompt.contains("conversation history"));
        assert!(prompt.contains("## Recent Conversation"));
        assert!(prompt.contains("What is Rust?"));
        assert!(prompt.contains("Rust is a systems programming language"));
        assert!(prompt.contains("Is it fast?"));
    }

    #[test]
    fn build_enriched_prompt_includes_context_snapshot() {
        use bus::events::ctp::ContextSnapshot;
        use bus::events::platform::{KeystrokeCadence, WindowContext};
        use std::time::Duration;

        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "VS Code".to_string(),
                window_title: Some("main.rs".into()),
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 95.5,
                burst_detected: false,
                idle_duration: Duration::from_secs(2),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(3600),
            inferred_task: None,
            timestamp: Instant::now(),
        };

        let prompt = InferenceActor::build_enriched_prompt("Help me", &[], Some(&snapshot), &[]);

        assert!(prompt.contains("## System"));
        assert!(prompt.contains("current context"));
        assert!(prompt.contains("## Current Context"));
        assert!(prompt.contains("VS Code"));
        assert!(prompt.contains("main.rs"));
        assert!(prompt.contains("Typing activity: 95"));
    }

    #[test]
    fn build_enriched_prompt_combines_all_sources() {
        use bus::events::ctp::ContextSnapshot;
        use bus::events::memory::MemoryChunk;
        use bus::events::platform::{KeystrokeCadence, WindowContext};

        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "Browser".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(1200),
            inferred_task: None,
            timestamp: Instant::now(),
        };

        let memory = vec![MemoryChunk {
            text: "Previous discussion about project X".into(),
            score: 0.88,
            timestamp: SystemTime::now(),
        }];

        let history = vec![(
            "What's project X?".into(),
            "It's our main initiative.".into(),
        )];

        let prompt = InferenceActor::build_enriched_prompt(
            "Continue discussing project X",
            &memory,
            Some(&snapshot),
            &history,
        );

        // Should have all sections in proper order
        let system_pos = prompt.find("## System").expect("should have system");
        let memory_pos = prompt
            .find("## Relevant Memory")
            .expect("should have memory");
        let history_pos = prompt
            .find("## Recent Conversation")
            .expect("should have history");
        let context_pos = prompt
            .find("## Current Context")
            .expect("should have context");
        let user_pos = prompt.find("## User").expect("should have user message");

        // System should come first
        assert!(system_pos < memory_pos);
        assert!(memory_pos < history_pos);
        assert!(history_pos < context_pos);
        assert!(context_pos < user_pos);

        // Verify system instruction mentions all available resources
        let system_section = &prompt[system_pos..memory_pos];
        assert!(system_section.contains("memory"));
        assert!(system_section.contains("conversation history"));
        assert!(system_section.contains("context"));
    }
}
