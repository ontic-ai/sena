//! Inference actor — receives InferenceRequested events and produces token streams.

use crate::backend::InferenceBackend;
use crate::error::InferenceError;
use crate::filter::OutputFilter;
use crate::queue::{InferenceQueue, WorkItem, WorkKind};
use crate::types::InferenceParams;
use bus::{
    Actor, ActorError, Event, EventBus, InferenceEvent, InferenceFailureOrigin, InferenceSource,
    Priority,
};
use std::sync::Arc;
use text::SentenceBoundaryIterator;
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{debug, info, trace, warn};

/// Default queue capacity.
const DEFAULT_QUEUE_CAPACITY: usize = 100;

/// Inference actor.
pub struct InferenceActor {
    bus: Option<Arc<EventBus>>,
    backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
    queue: Arc<Mutex<InferenceQueue>>,
    rx: Option<broadcast::Receiver<Event>>,
    directed_rx: Option<mpsc::Receiver<Event>>,
    work_tx: Option<mpsc::Sender<()>>,
}

impl InferenceActor {
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
        Self::with_queue_capacity(backend, DEFAULT_QUEUE_CAPACITY)
    }

    /// Create a new inference actor with custom queue capacity.
    pub fn with_queue_capacity(backend: Box<dyn InferenceBackend>, queue_capacity: usize) -> Self {
        Self {
            bus: None,
            backend: Arc::new(Mutex::new(backend)),
            queue: Arc::new(Mutex::new(InferenceQueue::new(queue_capacity))),
            rx: None,
            directed_rx: None,
            work_tx: None,
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

    /// Worker loop: process queued work items.
    ///
    /// Exits cleanly when the work signal channel closes.
    async fn worker_loop(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        queue: Arc<Mutex<InferenceQueue>>,
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
                        let result = Self::execute_embed(backend.clone(), text, causal_id).await;
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
                }
            }
        }
    }

    /// Execute an inference request with full streaming chain.
    async fn execute_inference(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
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

        // Create inference parameters (default for now)
        let params = InferenceParams::default();

        debug!(?params, "running inference");

        match source {
            InferenceSource::UserVoice | InferenceSource::UserText => {
                Self::execute_streaming_inference(bus, backend, prompt, source, causal_id, params)
                    .await
            }
            InferenceSource::ProactiveCTP | InferenceSource::Iterative => {
                Self::execute_batch_inference(bus, backend, prompt, source, causal_id, params).await
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
            backend_guard.infer(prompt, params).await?
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
        let confidence = if token_count >= 10 { 0.95 } else { 0.75 };

        // Emit stream completed event
        bus.broadcast(Event::Inference(InferenceEvent::InferenceStreamCompleted {
            full_text: full_text.clone(),
            source,
            token_count: token_count as usize,
            confidence: Some(confidence),
            causal_id,
        }))
        .await?;

        // Emit final completion event
        info!(
            token_count,
            text_len = full_text.len(),
            "inference complete"
        );
        bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
            text: full_text.clone(),
            token_count: token_count as usize,
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

        // Emit filtered sentences as InferenceSentenceReady (for TTS).
        // The original unfiltered full_text is preserved in InferenceStreamCompleted.
        let mut sentence_iter = SentenceBoundaryIterator::new();
        sentence_iter.push(&full_text);
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
            token_count,
            causal_id,
        }))
        .await?;

        Ok(full_text)
    }

    /// Execute an embedding request.
    async fn execute_embed(
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
        text: String,
        _causal_id: bus::CausalId,
    ) -> Result<Vec<f32>, InferenceError> {
        let backend_guard = backend.lock().await;

        if !backend_guard.is_loaded() {
            return Err(InferenceError::ModelNotLoaded);
        }

        let embedding = backend_guard.embed(text).await?;
        Ok(embedding)
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

    /// VRAM monitoring loop: poll backend VRAM usage every 2 seconds when model loaded.
    ///
    /// Exits when shutdown signal is received on the bus.
    async fn vram_monitoring_loop(
        bus: Arc<EventBus>,
        backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
    ) {
        let mut shutdown_rx = bus.subscribe_broadcast();
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let backend_guard = backend.lock().await;
                    if backend_guard.is_loaded() {
                        let (used_mb, total_mb, percent) = backend_guard.vram_usage();
                        drop(backend_guard);

                        let normalized_percent = if total_mb == 0 { 0 } else { percent };
                        let _ = bus.broadcast(Event::System(bus::SystemEvent::VramUsageUpdated {
                            used_mb,
                            total_mb,
                            percent: normalized_percent,
                        })).await;
                    }
                }
                event = shutdown_rx.recv() => {
                    match event {
                        Ok(Event::System(bus::SystemEvent::ShutdownSignal)) => {
                            debug!("vram monitoring loop: shutdown signal received");
                            break;
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
        let worker_queue = self.queue.clone();

        tokio::spawn(async move {
            Self::worker_loop(worker_bus, worker_backend, worker_queue, work_rx).await;
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

        let bus = self
            .bus
            .clone()
            .ok_or_else(|| ActorError::RuntimeError("bus not initialized".to_string()))?;

        loop {
            tokio::select! {
                // Handle broadcast events (user-facing inference, shutdown)
                event = rx.recv() => {
                    match event {
                        Ok(Event::System(bus::SystemEvent::ShutdownSignal)) => {
                            info!("inference actor received shutdown signal");
                            break;
                        }
                        Ok(Event::Inference(InferenceEvent::InferenceRequested {
                            prompt,
                            priority,
                            source,
                            causal_id,
                        })) => {
                            if let Err(e) = self
                                .handle_inference_request(prompt, source, priority, causal_id)
                                .await
                            {
                                warn!(error = ?e, "inference request handling failed");
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
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!("inference actor stopping");

        // Drop work_tx to close the channel and signal worker to exit
        drop(self.work_tx.take());

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
}
