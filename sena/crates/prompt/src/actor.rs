//! Prompt actor — bus-event-driven orchestrator for prompt assembly.
//!
//! `PromptActor` listens to context events from the bus (soul summaries, memory
//! query results, CTP snapshots) and assembles typed prompts using the
//! `PromptComposer` when a trigger event arrives.
//!
//! ## Trigger events
//! - `CTPEvent::ThoughtEventTriggered` — proactive inference with `Priority::Low`
//! - `SpeechEvent::TranscriptionCompleted` — user-voice inference with `Priority::High`
//!
//! ## Context cache events
//! - `SoulEvent::SummaryCompleted` — updates cached soul content
//! - `MemoryEvent::QueryCompleted` / `MemoryEvent::ContextQueryCompleted` — updates cached memory chunks
//! - `CTPEvent::ContextSnapshotReady` — updates cached CTP snapshot

use crate::composer::PromptComposer;
use crate::segment::{ProactiveDirective, PromptSegment};
use bus::events::ctp::ContextSnapshot;
use bus::events::memory::{ContextMemoryQueryResponse, ScoredChunk};
use bus::{
    Actor, ActorError, CTPEvent, CausalId, Event, EventBus, InferenceEvent, InferenceSource,
    MemoryEvent, Priority, SoulEvent, SoulSummary, SpeechEvent, SystemEvent,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

const PROMPT_MEMORY_QUERY_LIMIT: usize = 6;
const RECENT_DIALOGUE_TURN_LIMIT: usize = 6;

/// Configuration for the prompt actor.
#[derive(Debug, Clone, Default)]
pub struct PromptConfig {
    /// Word-count budget applied during prompt assembly.
    ///
    /// If `None`, no word limit is enforced.
    pub token_limit: Option<usize>,
}

/// Prompt actor — owns a composer and assembles prompts on bus events.
pub struct PromptActor {
    composer: PromptComposer,
    config: PromptConfig,
    bus: Option<Arc<EventBus>>,
    rx: Option<broadcast::Receiver<Event>>,
    // Context cache updated from bus events.
    cached_soul_content: Option<String>,
    cached_memory_chunks: Vec<ScoredChunk>,
    cached_snapshot: Option<Box<ContextSnapshot>>,
    pending_prompts: HashMap<CausalId, PendingPrompt>,
    pending_user_turns: HashMap<CausalId, String>,
    recent_dialogue_turns: VecDeque<String>,
}

enum PendingPrompt {
    Voice { user_text: String },
    Ctp { snapshot: ContextSnapshot },
}

impl PromptActor {
    /// Create a new prompt actor with the default composer and config.
    pub fn new() -> Self {
        Self {
            composer: PromptComposer::new(),
            config: PromptConfig::default(),
            bus: None,
            rx: None,
            cached_soul_content: None,
            cached_memory_chunks: Vec::new(),
            cached_snapshot: None,
            pending_prompts: HashMap::new(),
            pending_user_turns: HashMap::new(),
            recent_dialogue_turns: VecDeque::new(),
        }
    }

    /// Create a new prompt actor with explicit configuration.
    pub fn with_config(config: PromptConfig) -> Self {
        Self {
            composer: PromptComposer::new(),
            config,
            bus: None,
            rx: None,
            cached_soul_content: None,
            cached_memory_chunks: Vec::new(),
            cached_snapshot: None,
            pending_prompts: HashMap::new(),
            pending_user_turns: HashMap::new(),
            recent_dialogue_turns: VecDeque::new(),
        }
    }

    fn build_ctp_query(snapshot: &ContextSnapshot) -> String {
        let mut parts = vec![snapshot.active_app.app_name.clone()];

        if let Some(window_title) = &snapshot.active_app.window_title {
            parts.push(window_title.clone());
        }

        for event in snapshot.recent_files.iter().rev().take(3) {
            if let Some(file_name) = event.path.file_name().and_then(|name| name.to_str()) {
                parts.push(file_name.to_string());
            }
        }

        parts.join(" ")
    }

    /// Build prompt segments for a CTP-triggered proactive thought.
    fn build_ctp_segments_with_memory(
        &self,
        snapshot: &ContextSnapshot,
        memory_chunks: &[ScoredChunk],
    ) -> Vec<PromptSegment> {
        let mut segments = Vec::new();

        if let Some(content) = &self.cached_soul_content {
            if !content.is_empty() {
                segments.push(PromptSegment::SoulContext(SoulSummary {
                    content: content.clone(),
                    event_count: 0,
                    request_id: 0,
                }));
            }
        }

        if !memory_chunks.is_empty() {
            segments.push(PromptSegment::LongTermMemory(memory_chunks.to_vec()));
        }

        if !self.recent_dialogue_turns.is_empty() {
            segments.push(PromptSegment::WorkingMemorySnippets(
                self.recent_dialogue_turns.iter().cloned().collect(),
            ));
        }

        segments.push(PromptSegment::ProactiveResponseDirective(
            ProactiveDirective::ThreePathResponse,
        ));
        segments.push(PromptSegment::CurrentContext(Box::new(snapshot.clone())));
        segments
    }

    #[cfg(test)]
    fn build_ctp_segments(&self, snapshot: &ContextSnapshot) -> Vec<PromptSegment> {
        self.build_ctp_segments_with_memory(snapshot, &self.cached_memory_chunks)
    }

    /// Build prompt segments for a voice-transcription-triggered inference.
    fn build_voice_segments_with_memory(
        &self,
        user_text: &str,
        memory_chunks: &[ScoredChunk],
    ) -> Vec<PromptSegment> {
        let mut segments = Vec::new();

        if let Some(content) = &self.cached_soul_content {
            if !content.is_empty() {
                segments.push(PromptSegment::SoulContext(SoulSummary {
                    content: content.clone(),
                    event_count: 0,
                    request_id: 0,
                }));
            }
        }

        if let Some(snapshot) = &self.cached_snapshot {
            segments.push(PromptSegment::CurrentContext(snapshot.clone()));
        }

        if !memory_chunks.is_empty() {
            segments.push(PromptSegment::LongTermMemory(memory_chunks.to_vec()));
        }

        let mut working_memory: Vec<String> = self.recent_dialogue_turns.iter().cloned().collect();
        working_memory.push(format!("User: {}", user_text));
        segments.push(PromptSegment::WorkingMemorySnippets(working_memory));
        segments
    }

    #[cfg(test)]
    fn build_voice_segments(&self, user_text: &str) -> Vec<PromptSegment> {
        self.build_voice_segments_with_memory(user_text, &self.cached_memory_chunks)
    }

    async fn request_memory_for_prompt(
        &mut self,
        bus: &Arc<EventBus>,
        causal_id: CausalId,
        query: String,
        pending_prompt: PendingPrompt,
    ) {
        self.pending_prompts.insert(causal_id, pending_prompt);

        let _ = bus
            .broadcast(Event::Memory(MemoryEvent::MemoryQueryRequest {
                query,
                limit: PROMPT_MEMORY_QUERY_LIMIT,
                causal_id,
            }))
            .await;
    }

    async fn resolve_pending_prompt(
        &mut self,
        bus: &Arc<EventBus>,
        causal_id: CausalId,
        memory_chunks: Vec<ScoredChunk>,
    ) {
        let Some(pending_prompt) = self.pending_prompts.remove(&causal_id) else {
            return;
        };

        self.cached_memory_chunks = memory_chunks.clone();

        match pending_prompt {
            PendingPrompt::Voice { user_text } => {
                let segments = self.build_voice_segments_with_memory(&user_text, &memory_chunks);
                self.assemble_and_emit(
                    bus,
                    segments,
                    InferenceSource::UserVoice,
                    Priority::High,
                    causal_id,
                )
                .await;
            }
            PendingPrompt::Ctp { snapshot } => {
                let segments = self.build_ctp_segments_with_memory(&snapshot, &memory_chunks);
                self.assemble_and_emit(
                    bus,
                    segments,
                    InferenceSource::ProactiveCTP,
                    Priority::Low,
                    causal_id,
                )
                .await;
            }
        }
    }

    async fn fallback_pending_prompt(&mut self, bus: &Arc<EventBus>, causal_id: CausalId) {
        if !self.pending_prompts.contains_key(&causal_id) {
            return;
        }

        self.resolve_pending_prompt(bus, causal_id, self.cached_memory_chunks.clone())
            .await;
    }

    fn remember_dialogue_turn(&mut self, entry: String) {
        self.recent_dialogue_turns.push_back(entry);
        while self.recent_dialogue_turns.len() > RECENT_DIALOGUE_TURN_LIMIT {
            self.recent_dialogue_turns.pop_front();
        }
    }

    /// Assemble segments into a prompt and broadcast `InferenceRequested`.
    async fn assemble_and_emit(
        &self,
        bus: &Arc<EventBus>,
        segments: Vec<PromptSegment>,
        source: InferenceSource,
        priority: Priority,
        causal_id: CausalId,
    ) {
        let prompt = match self.config.token_limit {
            Some(limit) => self.composer.assemble_with_budget(&segments, limit),
            None => self.composer.assemble(&segments),
        };

        let prompt = match prompt {
            Ok(p) => p,
            Err(e) => {
                debug!(
                    error = %e,
                    ?source,
                    "prompt assembly produced no content — skipping inference emit"
                );
                return;
            }
        };

        debug!(
            prompt_len = prompt.len(),
            ?source,
            ?priority,
            ?causal_id,
            "PromptActor: emitting InferenceRequested"
        );

        let _ = bus
            .broadcast(Event::Inference(InferenceEvent::InferenceRequested {
                prompt,
                priority,
                source,
                causal_id,
            }))
            .await;
    }
}

impl Default for PromptActor {
    fn default() -> Self {
        Self::new()
    }
}

impl Actor for PromptActor {
    fn name(&self) -> &'static str {
        "prompt"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!(
            actor = self.name(),
            token_limit = ?self.config.token_limit,
            "PromptActor starting"
        );
        self.rx = Some(bus.subscribe_broadcast());
        self.bus = Some(bus);
        debug!(actor = self.name(), "PromptActor subscribed to bus");
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut rx = self.rx.take().ok_or_else(|| {
            ActorError::StartupFailed("rx not initialized — call start() first".to_string())
        })?;

        let bus = self.bus.clone().ok_or_else(|| {
            ActorError::StartupFailed("bus not initialized — call start() first".to_string())
        })?;

        info!(actor = self.name(), "PromptActor running");

        // Prime the soul cache so the first inference has context.
        let prime_cid = CausalId::new();
        let _ = bus
            .broadcast(Event::Soul(SoulEvent::SummaryRequested {
                max_events: 50,
                causal_id: prime_cid,
            }))
            .await;

        loop {
            match rx.recv().await {
                // --- Shutdown ---
                Ok(Event::System(SystemEvent::ShutdownSignal))
                | Ok(Event::System(SystemEvent::ShutdownRequested))
                | Ok(Event::System(SystemEvent::ShutdownInitiated)) => {
                    info!(actor = self.name(), "shutdown signal received — exiting");
                    break;
                }

                // --- Context cache: soul summary ---
                Ok(Event::Soul(SoulEvent::SummaryCompleted { content, .. })) => {
                    debug!(
                        actor = self.name(),
                        content_len = content.len(),
                        "soul summary cached"
                    );
                    self.cached_soul_content = Some(content);
                }
                Ok(Event::Soul(SoulEvent::PersonalityUpdated { causal_id, .. })) => {
                    debug!(
                        actor = self.name(),
                        ?causal_id,
                        "personality updated — refreshing soul summary cache"
                    );
                    let _ = bus
                        .broadcast(Event::Soul(SoulEvent::SummaryRequested {
                            max_events: 50,
                            causal_id,
                        }))
                        .await;
                }

                // --- Context cache: memory query results ---
                Ok(Event::Memory(MemoryEvent::QueryCompleted { chunks, causal_id })) => {
                    debug!(
                        actor = self.name(),
                        chunk_count = chunks.len(),
                        "memory query results cached"
                    );
                    self.resolve_pending_prompt(&bus, causal_id, chunks.clone())
                        .await;
                    self.cached_memory_chunks = chunks;
                }
                Ok(Event::Memory(MemoryEvent::MemoryQueryResponse { chunks, causal_id })) => {
                    debug!(
                        actor = self.name(),
                        chunk_count = chunks.len(),
                        ?causal_id,
                        "prompt memory query response received"
                    );
                    self.resolve_pending_prompt(&bus, causal_id, chunks).await;
                }
                Ok(Event::Memory(MemoryEvent::ContextQueryCompleted(
                    ContextMemoryQueryResponse { chunks, .. },
                ))) => {
                    debug!(
                        actor = self.name(),
                        chunk_count = chunks.len(),
                        "context memory query results cached"
                    );
                    self.cached_memory_chunks = chunks;
                }
                Ok(Event::Memory(MemoryEvent::QueryFailed { causal_id, .. })) => {
                    self.fallback_pending_prompt(&bus, causal_id).await;
                }
                Ok(Event::Inference(InferenceEvent::InferenceCompleted {
                    text,
                    source,
                    causal_id,
                    ..
                })) => {
                    if matches!(
                        source,
                        InferenceSource::UserVoice | InferenceSource::UserText
                    ) {
                        if let Some(user_text) = self.pending_user_turns.remove(&causal_id) {
                            self.remember_dialogue_turn(format!("User: {}", user_text));
                        }
                        if !text.trim().is_empty() {
                            self.remember_dialogue_turn(format!("Assistant: {}", text.trim()));
                        }
                    }
                }
                Ok(Event::Inference(InferenceEvent::InferenceFailed { causal_id, .. }))
                | Ok(Event::Inference(InferenceEvent::InferenceFailedWithOrigin {
                    causal_id,
                    ..
                })) => {
                    self.pending_user_turns.remove(&causal_id);
                }

                // --- Context cache and triggers from CTP ---
                Ok(Event::CTP(ctp_event)) => {
                    match *ctp_event {
                        CTPEvent::ContextSnapshotReady(snapshot) => {
                            debug!(actor = self.name(), "CTP snapshot cached");
                            self.cached_snapshot = Some(Box::new(snapshot));
                        }

                        // Trigger: proactive thought — assemble prompt and emit InferenceRequested.
                        CTPEvent::ThoughtEventTriggered(snapshot) => {
                            let causal_id = CausalId::new();
                            debug!(
                                actor = self.name(),
                                "ThoughtEventTriggered — assembling proactive prompt"
                            );
                            self.request_memory_for_prompt(
                                &bus,
                                causal_id,
                                Self::build_ctp_query(&snapshot),
                                PendingPrompt::Ctp { snapshot },
                            )
                            .await;
                        }

                        _ => {}
                    }
                }

                // --- Trigger: voice transcription — assemble prompt and emit InferenceRequested ---
                Ok(Event::Speech(SpeechEvent::TranscriptionCompleted {
                    text, causal_id, ..
                })) => {
                    self.pending_user_turns.insert(causal_id, text.clone());
                    debug!(
                        actor = self.name(),
                        text_len = text.len(),
                        ?causal_id,
                        "TranscriptionCompleted — requesting fresh memory before prompt assembly"
                    );
                    self.request_memory_for_prompt(
                        &bus,
                        causal_id,
                        text.clone(),
                        PendingPrompt::Voice { user_text: text },
                    )
                    .await;
                }

                Ok(_) => {
                    // Other events ignored.
                }

                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(actor = self.name(), lagged = n, "broadcast channel lagged");
                }

                Err(broadcast::error::RecvError::Closed) => {
                    debug!(actor = self.name(), "broadcast channel closed");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!(actor = self.name(), "PromptActor stopped");
        self.rx = None;
        self.bus = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::ContextSnapshot;
    use bus::events::memory::ScoredChunk;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use std::time::Duration;
    use std::time::Instant;
    use tokio::time::timeout;

    fn make_snapshot() -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "TestApp".to_string(),
                window_title: Some("test.rs".to_string()),
                bundle_id: None,
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 80.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(2),
                timestamp: now,
            },
            session_duration: Duration::from_secs(1800),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: now,
            soul_identity_signal: None,
        }
    }

    #[test]
    fn prompt_actor_constructs() {
        let actor = PromptActor::new();
        assert_eq!(actor.name(), "prompt");
    }

    #[test]
    fn prompt_actor_with_config() {
        let config = PromptConfig {
            token_limit: Some(4096),
        };
        let actor = PromptActor::with_config(config);
        assert_eq!(actor.config.token_limit, Some(4096));
    }

    #[test]
    fn prompt_actor_default() {
        let actor = PromptActor::default();
        assert_eq!(actor.name(), "prompt");
        assert_eq!(actor.config.token_limit, None);
    }

    #[test]
    fn build_ctp_segments_with_empty_cache_contains_context() {
        let actor = PromptActor::new();
        let snapshot = make_snapshot();
        let segments = actor.build_ctp_segments(&snapshot);
        // Should always include CurrentContext even with empty cache.
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::CurrentContext(_)))
        );
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::ProactiveResponseDirective(_)))
        );
    }

    #[test]
    fn build_ctp_segments_includes_soul_when_cached() {
        let mut actor = PromptActor::new();
        actor.cached_soul_content = Some("user prefers Rust".to_string());
        let snapshot = make_snapshot();
        let segments = actor.build_ctp_segments(&snapshot);
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::SoulContext(_)))
        );
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::CurrentContext(_)))
        );
    }

    #[test]
    fn build_ctp_segments_skips_empty_soul_content() {
        let mut actor = PromptActor::new();
        actor.cached_soul_content = Some(String::new());
        let snapshot = make_snapshot();
        let segments = actor.build_ctp_segments(&snapshot);
        // Empty soul content should not produce a SoulContext segment.
        assert!(
            !segments
                .iter()
                .any(|s| matches!(s, PromptSegment::SoulContext(_)))
        );
    }

    #[test]
    fn build_ctp_segments_includes_memory_when_cached() {
        let mut actor = PromptActor::new();
        actor.cached_memory_chunks = vec![ScoredChunk {
            content: "relevant fact".to_string(),
            score: 0.8,
            age_seconds: 3600,
        }];
        let snapshot = make_snapshot();
        let segments = actor.build_ctp_segments(&snapshot);
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::LongTermMemory(_)))
        );
    }

    #[test]
    fn build_voice_segments_always_includes_working_memory() {
        let actor = PromptActor::new();
        let segments = actor.build_voice_segments("hello world");
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::WorkingMemorySnippets(_)))
        );
    }

    #[test]
    fn build_voice_segments_includes_snapshot_when_cached() {
        let mut actor = PromptActor::new();
        actor.cached_snapshot = Some(Box::new(make_snapshot()));
        let segments = actor.build_voice_segments("test input");
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::CurrentContext(_)))
        );
        assert!(
            segments
                .iter()
                .any(|s| matches!(s, PromptSegment::WorkingMemorySnippets(_)))
        );
    }

    #[test]
    fn build_ctp_query_uses_snapshot_fields() {
        let snapshot = make_snapshot();
        let query = PromptActor::build_ctp_query(&snapshot);
        assert!(query.contains("TestApp"));
        assert!(query.contains("test.rs"));
    }

    #[tokio::test]
    async fn voice_trigger_requests_memory_before_inference() {
        let mut actor = PromptActor::new();
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(Arc::clone(&bus)).await.expect("start failed");
        let actor_handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        tokio::time::sleep(Duration::from_millis(25)).await;

        let causal_id = CausalId::new();
        bus.broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
            text: "remember prior context".to_string(),
            confidence: 0.95,
            causal_id,
        }))
        .await
        .expect("speech event broadcast failed");

        let mut saw_query_request = false;
        let mut inference_before_query_response = false;
        for _ in 0..20 {
            match timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(Event::Memory(MemoryEvent::MemoryQueryRequest {
                    query,
                    causal_id: response_cid,
                    ..
                }))) => {
                    assert_eq!(response_cid, causal_id);
                    assert!(query.contains("remember prior context"));
                    saw_query_request = true;
                    break;
                }
                Ok(Ok(Event::Inference(InferenceEvent::InferenceRequested { .. }))) => {
                    inference_before_query_response = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(
            saw_query_request,
            "voice trigger should issue a memory query request"
        );
        assert!(
            !inference_before_query_response,
            "inference should wait for a memory query response"
        );

        bus.broadcast(Event::Memory(MemoryEvent::MemoryQueryResponse {
            chunks: vec![ScoredChunk {
                content: "prior context chunk".to_string(),
                score: 0.9,
                age_seconds: 30,
            }],
            causal_id,
        }))
        .await
        .expect("memory response broadcast failed");

        let mut saw_inference_request = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Inference(InferenceEvent::InferenceRequested {
                prompt,
                source,
                causal_id: response_cid,
                ..
            }))) = timeout(Duration::from_millis(100), rx.recv()).await
            {
                assert_eq!(response_cid, causal_id);
                assert_eq!(source, InferenceSource::UserVoice);
                assert!(prompt.contains("prior context chunk"));
                saw_inference_request = true;
                break;
            }
        }

        assert!(
            saw_inference_request,
            "memory response should trigger inference"
        );

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast failed");
        let _ = timeout(Duration::from_secs(1), actor_handle).await;
    }

    #[tokio::test]
    async fn completed_voice_turn_is_cached_for_the_next_prompt() {
        let mut actor = PromptActor::new();
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(Arc::clone(&bus)).await.expect("start failed");
        let actor_handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        tokio::time::sleep(Duration::from_millis(25)).await;

        let first_causal_id = CausalId::new();
        bus.broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
            text: "hello sena".to_string(),
            confidence: 0.98,
            causal_id: first_causal_id,
        }))
        .await
        .expect("first speech event should broadcast");

        let mut saw_first_query = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Memory(MemoryEvent::MemoryQueryRequest { causal_id, .. }))) =
                timeout(Duration::from_millis(100), rx.recv()).await
            {
                if causal_id == first_causal_id {
                    saw_first_query = true;
                    break;
                }
            }
        }
        assert!(
            saw_first_query,
            "first voice turn should query memory before inference"
        );

        bus.broadcast(Event::Memory(MemoryEvent::MemoryQueryResponse {
            chunks: vec![],
            causal_id: first_causal_id,
        }))
        .await
        .expect("first memory response should broadcast");

        let mut saw_first_inference = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Inference(InferenceEvent::InferenceRequested {
                causal_id, ..
            }))) = timeout(Duration::from_millis(100), rx.recv()).await
            {
                if causal_id == first_causal_id {
                    saw_first_inference = true;
                    break;
                }
            }
        }
        assert!(
            saw_first_inference,
            "first voice turn should emit inference request"
        );

        bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
            text: "hello human".to_string(),
            source: InferenceSource::UserVoice,
            token_count: 2,
            causal_id: first_causal_id,
        }))
        .await
        .expect("first completion should broadcast");

        let second_causal_id = CausalId::new();
        bus.broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
            text: "what did you just say".to_string(),
            confidence: 0.98,
            causal_id: second_causal_id,
        }))
        .await
        .expect("second speech event should broadcast");

        let mut saw_second_query = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Memory(MemoryEvent::MemoryQueryRequest { causal_id, .. }))) =
                timeout(Duration::from_millis(100), rx.recv()).await
            {
                if causal_id == second_causal_id {
                    saw_second_query = true;
                    break;
                }
            }
        }
        assert!(
            saw_second_query,
            "second voice turn should query memory before inference"
        );

        bus.broadcast(Event::Memory(MemoryEvent::MemoryQueryResponse {
            chunks: vec![],
            causal_id: second_causal_id,
        }))
        .await
        .expect("second memory response should broadcast");

        let mut saw_second_inference = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Inference(InferenceEvent::InferenceRequested {
                prompt,
                causal_id,
                ..
            }))) = timeout(Duration::from_millis(100), rx.recv()).await
            {
                if causal_id == second_causal_id {
                    assert!(prompt.contains("User: hello sena"));
                    assert!(prompt.contains("Assistant: hello human"));
                    assert!(prompt.contains("User: what did you just say"));
                    saw_second_inference = true;
                    break;
                }
            }
        }

        assert!(
            saw_second_inference,
            "the next voice prompt should include recent dialogue history"
        );

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast failed");
        let _ = timeout(Duration::from_secs(1), actor_handle).await;
    }
}
