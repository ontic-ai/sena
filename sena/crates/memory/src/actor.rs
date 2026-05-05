//! Memory actor: manages memory ingestion, consolidation, and retrieval.

use crate::backend::MemoryBackend;
use crate::error::MemoryError;
use bus::events::{
    DistilledIdentitySignal, MemoryEvent, MemoryKind, ScoredChunk, SystemEvent,
    TemporalBehaviorPattern,
};
use bus::{
    Actor, ActorError, CausalId, Event, EventBus, InferenceEvent, SoulEvent, SpeechEvent,
    TransparencyEvent, TransparencyQuery, TransparencyResult,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

const USER_MEMORY_TRANSPARENCY_QUERY: &str = "recent important memories and observations";
const USER_MEMORY_TRANSPARENCY_LIMIT: usize = 8;

/// Minimum temporal pattern strength to include in transparency summary.
const TEMPORAL_PATTERN_STRENGTH_THRESHOLD: f64 = 0.3;

/// Maximum number of items to include in each transparency summary category.
const TRANSPARENCY_SUMMARY_MAX_ITEMS: usize = 5;

/// In-memory cache of soul personalization state, populated by listening to soul bus events.
///
/// The memory actor observes soul events emitted by [`SoulActor`] and caches the
/// data it needs to populate [`bus::events::transparency::SoulSummary`] in response
/// to `TransparencyQuery::UserMemory`. No direct cross-actor calls are made — all
/// data arrives through the broadcast bus.
#[derive(Default)]
struct SoulPersonalityCache {
    /// Cumulative count of soul events logged.
    ///
    /// Maps to `SoulSummary::inference_cycle_count` — represents how many observations
    /// the soul has accumulated, reflecting the depth of learned personalization.
    event_log_count: usize,

    /// Tool preferences collected from identity signals.
    ///
    /// Populated by:
    /// - `DistilledIdentitySignal` with `signal_key == "frequent_app"` → `signal_value`
    /// - `DistilledIdentitySignal` with `signal_key` starting with `"tool_pref::"` → stripped value
    ///
    /// Deduplication is applied on insert to keep the list compact.
    tool_preferences: Vec<String>,

    /// Interest clusters collected from identity signals.
    ///
    /// Populated by `DistilledIdentitySignal` with `signal_key` starting with `"interest::"`
    /// → the `signal_value` field.
    ///
    /// Deduplication is applied on insert.
    interest_clusters: Vec<String>,

    /// Work patterns collected from temporal behavior and identity signals.
    ///
    /// Populated by:
    /// - `TemporalBehaviorPattern.pattern_type` (when strength ≥ threshold)
    /// - `DistilledIdentitySignal` with `signal_key` starting with `"work_pattern::"` → `signal_value`
    ///
    /// Deduplication is applied on insert.
    work_patterns: Vec<String>,
}

impl std::fmt::Debug for SoulPersonalityCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoulPersonalityCache")
            .field("event_log_count", &self.event_log_count)
            .field(
                "tool_preferences",
                &format!("[{} items]", self.tool_preferences.len()),
            )
            .field(
                "interest_clusters",
                &format!("[{} items]", self.interest_clusters.len()),
            )
            .field(
                "work_patterns",
                &format!("[{} items]", self.work_patterns.len()),
            )
            .finish()
    }
}

impl SoulPersonalityCache {
    /// Push a value into a list if it is not already present and the list is under the size cap.
    fn push_deduped(list: &mut Vec<String>, value: String) {
        if !list.contains(&value) && list.len() < TRANSPARENCY_SUMMARY_MAX_ITEMS {
            list.push(value);
        }
    }

    /// Update cache from a distilled identity signal broadcast by `SoulActor`.
    fn update_from_identity_signal(&mut self, signal: &DistilledIdentitySignal) {
        let key = signal.signal_key.as_str();
        let value = signal.signal_value.clone();

        if key == "frequent_app" {
            Self::push_deduped(&mut self.tool_preferences, value);
        } else if let Some(tool) = key.strip_prefix("tool_pref::") {
            Self::push_deduped(&mut self.tool_preferences, tool.to_string());
        } else if key.starts_with("interest::") {
            Self::push_deduped(&mut self.interest_clusters, value);
        } else if let Some(pattern) = key.strip_prefix("work_pattern::") {
            Self::push_deduped(&mut self.work_patterns, pattern.to_string());
        }
    }

    /// Update cache from a temporal behavior pattern broadcast by `SoulActor`.
    ///
    /// Only patterns whose strength meets the minimum threshold are included.
    fn update_from_temporal_pattern(&mut self, pattern: &TemporalBehaviorPattern) {
        if pattern.strength >= TEMPORAL_PATTERN_STRENGTH_THRESHOLD {
            Self::push_deduped(&mut self.work_patterns, pattern.pattern_type.clone());
        }
    }

    /// Build the transparency `SoulSummary` from the current cache.
    fn to_soul_summary(&self) -> bus::events::transparency::SoulSummary {
        bus::events::transparency::SoulSummary {
            inference_cycle_count: self.event_log_count,
            work_patterns: self.work_patterns.clone(),
            tool_preferences: self.tool_preferences.clone(),
            interest_clusters: self.interest_clusters.clone(),
        }
    }
}

/// Backup policy for the memory store.
#[derive(Debug, Clone)]
pub struct BackupConfig {
    /// Whether backups are enabled.
    pub enabled: bool,
    /// Directory where backup files are written.
    pub path: PathBuf,
    /// Maximum number of backup files to retain. Older files are deleted.
    pub keep_last_n: usize,
}

impl BackupConfig {
    /// Construct a default backup config using the given data directory.
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            enabled: true,
            path: data_dir.join("backups"),
            keep_last_n: 3,
        }
    }
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: PathBuf::from("backups"),
            keep_last_n: 3,
        }
    }
}

/// Memory actor state.
pub struct MemoryActor {
    bus: Option<Arc<EventBus>>,
    backend: Box<dyn MemoryBackend>,
    backup_config: BackupConfig,
    pending_user_utterances: HashMap<CausalId, String>,
    /// Cache of soul personalization data observed through bus events.
    soul_cache: SoulPersonalityCache,
}

impl MemoryActor {
    /// Create a new memory actor with the given backend.
    pub fn new(backend: Box<dyn MemoryBackend>) -> Self {
        Self {
            bus: None,
            backend,
            backup_config: BackupConfig::default(),
            pending_user_utterances: HashMap::new(),
            soul_cache: SoulPersonalityCache::default(),
        }
    }

    /// Create a new memory actor with the given backend and backup configuration.
    pub fn with_backup_config(
        backend: Box<dyn MemoryBackend>,
        backup_config: BackupConfig,
    ) -> Self {
        Self {
            bus: None,
            backend,
            backup_config,
            pending_user_utterances: HashMap::new(),
            soul_cache: SoulPersonalityCache::default(),
        }
    }

    fn decorate_memory_write(&mut self, text: String, causal_id: CausalId) -> String {
        match self.pending_user_utterances.remove(&causal_id) {
            Some(user_text) => format!("User: {}\nAssistant: {}", user_text.trim(), text.trim()),
            None => text,
        }
    }

    /// Handle memory ingestion request.
    async fn handle_ingest_request(
        &mut self,
        text: String,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError> {
        debug!(
            text_len = text.len(),
            ?kind,
            causal_id = causal_id.as_u64(),
            "handling ingest request"
        );

        self.backend.ingest(&text, kind, causal_id).await?;

        debug!(
            causal_id = causal_id.as_u64(),
            "ingest completed successfully"
        );

        Ok(())
    }

    /// Handle memory query request.
    async fn handle_query_request(
        &self,
        query: String,
        limit: usize,
        causal_id: CausalId,
    ) -> Result<Vec<ScoredChunk>, MemoryError> {
        debug!(
            query_len = query.len(),
            limit,
            causal_id = causal_id.as_u64(),
            "handling query request"
        );

        let chunks = self.backend.query(&query, limit).await?;

        debug!(
            causal_id = causal_id.as_u64(),
            chunk_count = chunks.len(),
            "query completed successfully"
        );

        Ok(chunks)
    }

    /// Handle context memory query request from CTP.
    ///
    /// This is distinct from user-initiated queries in that it includes
    /// aggregate relevance scoring to help CTP assess the overall memory utility.
    async fn handle_context_query_request(
        &self,
        request: bus::events::memory::ContextMemoryQueryRequest,
    ) -> Result<bus::events::memory::ContextMemoryQueryResponse, MemoryError> {
        debug!(
            context_len = request.context_description.len(),
            max_chunks = request.max_chunks,
            causal_id = request.causal_id.as_u64(),
            "handling context query request"
        );

        let chunks = self
            .backend
            .query(&request.context_description, request.max_chunks)
            .await?;

        // Calculate aggregate relevance score as mean of chunk scores, or 0.0 if empty
        let relevance_score = if chunks.is_empty() {
            0.0
        } else {
            chunks.iter().map(|c| c.score as f64).sum::<f64>() / chunks.len() as f64
        };

        debug!(
            causal_id = request.causal_id.as_u64(),
            chunk_count = chunks.len(),
            relevance_score = relevance_score,
            "context query completed successfully"
        );

        Ok(bus::events::memory::ContextMemoryQueryResponse {
            chunks,
            relevance_score,
            causal_id: request.causal_id,
        })
    }

    async fn handle_user_memory_transparency_query(
        &self,
        bus: &Arc<EventBus>,
    ) -> Result<(), ActorError> {
        let chunks = match self
            .backend
            .query(
                USER_MEMORY_TRANSPARENCY_QUERY,
                USER_MEMORY_TRANSPARENCY_LIMIT,
            )
            .await
        {
            Ok(chunks) => chunks
                .into_iter()
                .map(|chunk| bus::events::MemoryChunk {
                    content: chunk.content,
                    score: chunk.score,
                    age_seconds: chunk.age_seconds,
                })
                .collect(),
            Err(e) => {
                warn!(error = %e, "user memory transparency query failed, returning empty result");
                Vec::new()
            }
        };

        // Build soul summary from the personalization cache populated via bus events.
        // The cache is updated incrementally as SoulActor emits identity and temporal signals.
        let soul_summary = self.soul_cache.to_soul_summary();

        let response = bus::events::transparency::MemoryResponse {
            soul_summary,
            memory_chunks: chunks,
        };

        bus.broadcast(Event::Transparency(TransparencyEvent::QueryResponse {
            query: TransparencyQuery::UserMemory,
            result: Box::new(TransparencyResult::Memory(response)),
        }))
        .await
        .map_err(|e| ActorError::RuntimeError(format!("broadcast failed: {}", e)))
    }

    /// Perform a backup of working memory state to the configured backup directory.
    ///
    /// Writes a JSON summary of metadata to a timestamped file.
    /// Returns the path of the backup file written.
    async fn perform_backup(&self, _causal_id: CausalId) -> Result<PathBuf, String> {
        if !self.backup_config.enabled {
            return Err("backup disabled by config".to_string());
        }

        // Ensure backup directory exists
        std::fs::create_dir_all(&self.backup_config.path)
            .map_err(|e| format!("failed to create backup directory: {e}"))?;

        // Generate timestamped filename
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let filename = format!("memory-backup-{}.json", now.as_secs());
        let backup_path = self.backup_config.path.join(&filename);

        // Validate the backup path stays within the configured backup dir
        if !backup_path.starts_with(&self.backup_config.path) {
            return Err(format!(
                "Backup path {:?} is outside configured directory {:?}",
                backup_path, self.backup_config.path
            ));
        }

        self.backend
            .export_json(backup_path.clone())
            .await
            .map_err(|e| format!("failed to export memory snapshot: {e}"))?;

        // Set restrictive permissions on Unix systems to prevent other OS users from reading
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&backup_path, perms) {
                warn!(error = %e, "Failed to set restrictive permissions on backup file");
            }
        }

        info!(path = ?backup_path, "memory backup written");

        // Prune old backups, keeping only the last N
        if let Err(e) = self.prune_old_backups() {
            warn!(error = %e, "failed to prune old backups (non-fatal)");
        }

        Ok(backup_path)
    }

    /// Delete oldest backup files if count exceeds `keep_last_n`.
    fn prune_old_backups(&self) -> Result<(), String> {
        let dir = &self.backup_config.path;

        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("failed to read backup directory: {e}"))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("memory-backup-")
            })
            .collect();

        if entries.len() <= self.backup_config.keep_last_n {
            return Ok(());
        }

        // Sort by name (which encodes timestamp) ascending so oldest come first
        entries.sort_by_key(|e| e.file_name());

        let to_delete = entries.len() - self.backup_config.keep_last_n;
        for entry in entries.into_iter().take(to_delete) {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                warn!(path = ?entry.path(), error = %e, "failed to delete old backup");
            }
        }

        Ok(())
    }
}

impl Actor for MemoryActor {
    fn name(&self) -> &'static str {
        "memory"
    }

    #[allow(clippy::manual_async_fn)]
    fn start(
        &mut self,
        bus: Arc<EventBus>,
    ) -> impl std::future::Future<Output = Result<(), ActorError>> + Send {
        async move {
            info!("memory actor starting");
            self.bus = Some(bus);
            Ok(())
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn run(&mut self) -> impl std::future::Future<Output = Result<(), ActorError>> + Send {
        async {
            let bus = self
                .bus
                .as_ref()
                .ok_or_else(|| ActorError::StartupFailed("bus not initialized".to_string()))?
                .clone();

            let mut rx = bus.subscribe_broadcast();
            let mut consolidation_tick = tokio::time::interval(std::time::Duration::from_secs(300)); // Every 5 minutes
            let mut consolidation_enabled = true;
            let mut shutdown_backup_completed = false;

            // Broadcast initial consolidation loop status
            let _ = bus
                .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                    loop_name: "memory_consolidation".to_string(),
                    enabled: true,
                }))
                .await;

            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                                info!("shutdown signal received, stopping memory actor");
                                break;
                            }
                            Ok(Event::System(SystemEvent::ShutdownRequested))
                            | Ok(Event::System(SystemEvent::ShutdownInitiated)) if !shutdown_backup_completed => {
                                info!("shutdown initiated — performing memory backup");
                                let causal_id = CausalId::new();
                                match self.perform_backup(causal_id).await {
                                    Ok(path) => {
                                        shutdown_backup_completed = true;
                                        let _ =
                                            bus.broadcast(Event::Memory(
                                                MemoryEvent::BackupCompleted { path, causal_id },
                                            ))
                                            .await;
                                    }
                                    Err(reason) => {
                                        shutdown_backup_completed = true;
                                        error!(error = %reason, "memory backup failed");
                                        let _ = bus
                                            .broadcast(Event::Memory(MemoryEvent::BackupFailed {
                                                reason,
                                                causal_id,
                                            }))
                                            .await;
                                    }
                                }
                            }
                            Ok(Event::System(SystemEvent::LoopControlRequested {
                                loop_name,
                                enabled,
                            })) if loop_name == "memory_consolidation" => {
                                info!(
                                    enabled = enabled,
                                    "memory consolidation loop control requested"
                                );
                                consolidation_enabled = enabled;

                                // Broadcast status changed event
                                let _ = bus
                                    .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                                        loop_name: "memory_consolidation".to_string(),
                                        enabled,
                                    }))
                                    .await;
                            }
                            Ok(Event::Transparency(TransparencyEvent::QueryRequested(
                                TransparencyQuery::UserMemory,
                            ))) => {
                                if let Err(e) = self.handle_user_memory_transparency_query(&bus).await {
                                    warn!(error = ?e, "user memory transparency response failed");
                                }
                            }
                            Ok(Event::Memory(memory_event)) => match memory_event {
                                MemoryEvent::StatsRequested { causal_id } => {
                                    match self.backend.stats().await {
                                        Ok(stats) => {
                                            bus.broadcast(Event::Memory(MemoryEvent::StatsCompleted {
                                                working_memory_chunks: stats.working_memory_chunks,
                                                long_term_memory_nodes: stats.long_term_memory_nodes,
                                                causal_id,
                                            }))
                                            .await
                                            .map_err(|e| {
                                                ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                            })?;
                                        }
                                        Err(e) => {
                                            bus.broadcast(Event::Memory(MemoryEvent::StatsFailed {
                                                causal_id,
                                                reason: e.to_string(),
                                            }))
                                            .await
                                            .map_err(|e| {
                                                ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                            })?;
                                        }
                                    }
                                }
                                MemoryEvent::IngestRequested {
                                    text,
                                    kind,
                                    causal_id,
                                } => {
                                    if let Err(e) =
                                        self.handle_ingest_request(text, kind, causal_id).await
                                    {
                                        error!(
                                            causal_id = causal_id.as_u64(),
                                            error = %e,
                                            "ingest failed"
                                        );
                                        bus.broadcast(Event::Memory(MemoryEvent::IngestFailed {
                                            causal_id,
                                            reason: e.to_string(),
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    } else {
                                        bus.broadcast(Event::Memory(MemoryEvent::IngestCompleted {
                                            causal_id,
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    }
                                }
                                MemoryEvent::MemoryWriteRequest { text, kind, causal_id } => {
                                    let text = self.decorate_memory_write(text, causal_id);
                                    if let Err(e) =
                                        self.handle_ingest_request(text, kind, causal_id).await
                                    {
                                        error!(
                                            causal_id = causal_id.as_u64(),
                                            error = %e,
                                            "memory write failed"
                                        );
                                        bus.broadcast(Event::Memory(MemoryEvent::MemoryWriteFailed {
                                            causal_id,
                                            reason: e.to_string(),
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    } else {
                                        bus.broadcast(Event::Memory(MemoryEvent::MemoryWriteCompleted {
                                            causal_id,
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    }
                                }
                                MemoryEvent::QueryRequested {
                                    query,
                                    limit,
                                    causal_id,
                                } => match self.handle_query_request(query, limit, causal_id).await {
                                    Ok(chunks) => {
                                        bus.broadcast(Event::Memory(MemoryEvent::QueryCompleted {
                                            chunks,
                                            causal_id,
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    }
                                    Err(e) => {
                                        error!(
                                            causal_id = causal_id.as_u64(),
                                            error = %e,
                                            "query failed"
                                        );
                                        bus.broadcast(Event::Memory(MemoryEvent::QueryFailed {
                                            causal_id,
                                            reason: e.to_string(),
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    }
                                },
                                MemoryEvent::MemoryQueryRequest {
                                    query,
                                    limit,
                                    causal_id,
                                } => match self.handle_query_request(query, limit, causal_id).await {
                                    Ok(chunks) => {
                                        bus.broadcast(Event::Memory(MemoryEvent::MemoryQueryResponse {
                                            chunks,
                                            causal_id,
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    }
                                    Err(e) => {
                                        error!(
                                            causal_id = causal_id.as_u64(),
                                            error = %e,
                                            "memory query failed"
                                        );
                                        bus.broadcast(Event::Memory(MemoryEvent::QueryFailed {
                                            causal_id,
                                            reason: e.to_string(),
                                        }))
                                        .await
                                        .map_err(|e| {
                                            ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                        })?;
                                    }
                                },
                                MemoryEvent::ContextQueryRequested(request) => {
                                    let causal_id = request.causal_id; // Preserve for error case
                                    match self.handle_context_query_request(request).await {
                                        Ok(response) => {
                                            bus.broadcast(Event::Memory(
                                                MemoryEvent::ContextQueryCompleted(response),
                                            ))
                                            .await
                                            .map_err(|e| {
                                                ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                            })?;
                                        }
                                        Err(e) => {
                                            error!(
                                                causal_id = causal_id.as_u64(),
                                                error = %e,
                                                "context query failed"
                                            );
                                            bus.broadcast(Event::Memory(MemoryEvent::ContextQueryFailed {
                                                causal_id,
                                                reason: e.to_string(),
                                            }))
                                            .await
                                            .map_err(|e| {
                                                ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                            })?;
                                        }
                                    }
                                }
                                _ => {}
                            },
                            // ── Soul events ──────────────────────────────────────────────────────
                            // The memory actor observes soul events to keep its personalization
                            // cache current. No direct cross-actor calls are made; all data
                            // arrives through the broadcast bus.
                            Ok(Event::Soul(SoulEvent::EventLogged { .. })) => {
                                self.soul_cache.event_log_count =
                                    self.soul_cache.event_log_count.saturating_add(1);
                                debug!(
                                    count = self.soul_cache.event_log_count,
                                    "soul_cache: event_log_count updated"
                                );
                            }
                            Ok(Event::Soul(SoulEvent::IdentitySignalDistilled { signal, .. })) => {
                                debug!(
                                    signal_key = %signal.signal_key,
                                    "soul_cache: identity signal received"
                                );
                                self.soul_cache.update_from_identity_signal(&signal);
                            }
                            Ok(Event::Soul(SoulEvent::TemporalPatternDetected {
                                pattern, ..
                            })) => {
                                debug!(
                                    pattern_type = %pattern.pattern_type,
                                    strength = pattern.strength,
                                    "soul_cache: temporal pattern received"
                                );
                                self.soul_cache.update_from_temporal_pattern(&pattern);
                            }
                            Ok(Event::Soul(SoulEvent::Deleted { .. })) => {
                                self.soul_cache = SoulPersonalityCache::default();
                                debug!("soul_cache: cleared after soul deletion");
                            }
                            Ok(Event::Speech(SpeechEvent::TranscriptionCompleted {
                                text,
                                causal_id,
                                ..
                            })) => {
                                self.pending_user_utterances.insert(causal_id, text);
                            }
                            Ok(Event::Inference(InferenceEvent::InferenceFailed { causal_id, .. }))
                            | Ok(Event::Inference(InferenceEvent::InferenceFailedWithOrigin {
                                causal_id,
                                ..
                            })) => {
                                self.pending_user_utterances.remove(&causal_id);
                            }
                            Ok(_) => {}
                            Err(e) => {
                                error!(error = %e, "broadcast channel error");
                                return Err(ActorError::ChannelClosed(e.to_string()));
                            }
                        }
                    }

                    _ = consolidation_tick.tick() => {
                        if !consolidation_enabled {
                            continue;
                        }

                        debug!("running periodic memory consolidation");

                        // Perform real consolidation by calling backend maintenance methods
                        // This is lightweight work that happens in the background periodically
                        match self.backend.consolidate().await {
                            Ok(nodes_affected) => {
                                debug!(
                                    nodes_affected = nodes_affected,
                                    "memory consolidation completed"
                                );
                                let _ = bus
                                    .broadcast(Event::Memory(MemoryEvent::ConsolidationCompleted {
                                        nodes_decayed: nodes_affected,
                                    }))
                                    .await;
                            }
                            Err(e) => {
                                warn!(error = %e, "memory consolidation failed (non-fatal)");
                                // Still emit completion event with 0 nodes to indicate
                                // the consolidation cycle ran (even if it failed)
                                let _ = bus
                                    .broadcast(Event::Memory(MemoryEvent::ConsolidationCompleted {
                                        nodes_decayed: 0,
                                    }))
                                    .await;
                            }
                        }
                    }
                }
            }

            info!("memory actor stopped");
            Ok(())
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn stop(&mut self) -> impl std::future::Future<Output = Result<(), ActorError>> + Send {
        async {
            info!("memory actor cleanup complete");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::StubBackend;
    use crate::embedder::{EMBEDDING_DIMENSIONS, SenaEmbedder};
    use inference::EmbedRequest;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    fn test_embedding(text: &str) -> Vec<f32> {
        let mut vector = vec![0.0_f32; EMBEDDING_DIMENSIONS];
        for token in text.to_lowercase().split_whitespace() {
            let slot = match token {
                "rust" => 0,
                "important" => 1,
                "world" => 2,
                other => {
                    3 + (other.bytes().fold(0_u64, |acc, byte| acc + byte as u64) as usize
                        % (EMBEDDING_DIMENSIONS.saturating_sub(3).max(1)))
                }
            };
            vector[slot] += 1.0;
        }

        if vector.iter().all(|value| *value == 0.0) {
            vector[EMBEDDING_DIMENSIONS - 1] = 1.0;
        }

        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }

        vector
    }

    fn spawn_embed_sender() -> mpsc::Sender<EmbedRequest> {
        let (embed_tx, mut embed_rx) = mpsc::channel::<EmbedRequest>(8);
        tokio::spawn(async move {
            while let Some(request) = embed_rx.recv().await {
                let _ = request.response_tx.send(Ok(test_embedding(&request.text)));
            }
        });
        embed_tx
    }

    fn persistent_backend(temp_dir: &tempfile::TempDir) -> Box<dyn MemoryBackend> {
        let embedder = SenaEmbedder::new(spawn_embed_sender());
        Box::new(
            crate::echo0_backend::Echo0Backend::open(
                &temp_dir.path().join("memory.redb"),
                embedder,
            )
            .expect("persistent backend should open"),
        )
    }

    #[test]
    fn memory_actor_constructs_with_backend() {
        let backend = Box::new(StubBackend::new());
        let actor = MemoryActor::new(backend);
        assert_eq!(actor.name(), "memory");
    }

    #[tokio::test]
    async fn memory_actor_lifecycle_completes() {
        let backend = Box::new(StubBackend::new());
        let mut actor = MemoryActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor.start(Arc::clone(&bus)).await.expect("start failed");
        actor.stop().await.expect("stop failed");
    }

    #[tokio::test]
    async fn memory_actor_handles_ingest_through_backend() {
        let backend = Box::new(StubBackend::new());
        let mut actor = MemoryActor::new(backend);

        let result = actor
            .handle_ingest_request(
                "test memory".to_string(),
                MemoryKind::Episodic,
                CausalId::new(),
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn memory_actor_handles_query_through_backend() {
        let backend = Box::new(StubBackend::new());
        let actor = MemoryActor::new(backend);

        let result = actor
            .handle_query_request("test query".to_string(), 10, CausalId::new())
            .await;

        assert!(result.is_ok());
        let chunks = result.unwrap();
        assert!(chunks.is_empty(), "stub backend returns empty chunks");
    }

    #[tokio::test]
    async fn backup_creates_file_in_configured_directory() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let backup_dir = dir.path().join("backups");

        let backend = Box::new(StubBackend::new());
        let config = BackupConfig {
            enabled: true,
            path: backup_dir.clone(),
            keep_last_n: 3,
        };
        let actor = MemoryActor::with_backup_config(backend, config);

        let causal_id = CausalId::new();
        let result = actor.perform_backup(causal_id).await;

        assert!(result.is_ok(), "backup should succeed: {:?}", result.err());
        let backup_path = result.unwrap();
        assert!(
            backup_path.exists(),
            "backup file should exist at {:?}",
            backup_path
        );
        assert!(backup_path.starts_with(&backup_dir));
    }

    #[tokio::test]
    async fn backup_disabled_returns_error() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let backend = Box::new(StubBackend::new());
        let config = BackupConfig {
            enabled: false,
            path: dir.path().join("backups"),
            keep_last_n: 3,
        };
        let actor = MemoryActor::with_backup_config(backend, config);

        let result = actor.perform_backup(CausalId::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("disabled"));
    }

    #[tokio::test]
    async fn backup_prunes_old_files_when_over_limit() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Pre-populate with 4 old backup files
        for i in 0u64..4 {
            let path = backup_dir.join(format!("memory-backup-{i}.json"));
            std::fs::write(&path, "{}").unwrap();
        }

        let backend = Box::new(StubBackend::new());
        let config = BackupConfig {
            enabled: true,
            path: backup_dir.clone(),
            keep_last_n: 3,
        };
        let actor = MemoryActor::with_backup_config(backend, config);

        // Trigger a backup which will also prune
        actor
            .perform_backup(CausalId::new())
            .await
            .expect("backup failed");

        // Count remaining backup files
        let count = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("memory-backup-")
            })
            .count();

        assert!(
            count <= 3,
            "expected at most 3 backups after pruning, got {count}"
        );
    }

    #[tokio::test]
    async fn consolidation_loop_responds_to_control_events() {
        use tokio::time::{Duration, sleep};

        let backend = Box::new(StubBackend::new());
        let mut actor = MemoryActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        // Spawn actor run in background
        let actor_bus = Arc::clone(&bus);
        let handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Wait for initial LoopStatusChanged event
        let mut got_initial_status = false;
        for _ in 0..10 {
            match rx.try_recv() {
                Ok(Event::System(SystemEvent::LoopStatusChanged { loop_name, enabled }))
                    if loop_name == "memory_consolidation" =>
                {
                    assert!(enabled, "initial state should be enabled");
                    got_initial_status = true;
                    break;
                }
                _ => {}
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert!(got_initial_status, "should emit initial loop status");

        // Send disable request
        actor_bus
            .broadcast(Event::System(SystemEvent::LoopControlRequested {
                loop_name: "memory_consolidation".to_string(),
                enabled: false,
            }))
            .await
            .expect("broadcast failed");

        // Wait for status changed event
        let mut got_disabled = false;
        for _ in 0..10 {
            match rx.try_recv() {
                Ok(Event::System(SystemEvent::LoopStatusChanged { loop_name, enabled }))
                    if loop_name == "memory_consolidation" =>
                {
                    assert!(!enabled, "state should be disabled");
                    got_disabled = true;
                    break;
                }
                _ => {}
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert!(got_disabled, "should respond to disable request");

        // Cleanup
        actor_bus
            .broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast failed");
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn consolidation_performs_real_maintenance_with_echo0_backend() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let backend = persistent_backend(&temp_dir);
        let mut actor = MemoryActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        // Ingest some test chunks
        actor
            .handle_ingest_request(
                "important information".to_string(),
                MemoryKind::Episodic,
                CausalId::new(),
            )
            .await
            .expect("ingest failed");

        actor
            .handle_ingest_request(
                "more important data".to_string(),
                MemoryKind::Semantic,
                CausalId::new(),
            )
            .await
            .expect("ingest failed");

        // Query before consolidation
        let chunks_before = actor
            .handle_query_request("important".to_string(), 10, CausalId::new())
            .await
            .expect("query failed");
        assert_eq!(chunks_before.len(), 2, "should find both chunks");
        assert_eq!(chunks_before[0].score, 1.0, "fresh chunks have score 1.0");

        // Perform consolidation
        let result = actor.backend.consolidate().await;
        assert!(result.is_ok(), "consolidation should succeed");
        let affected = result.unwrap();
        assert_eq!(affected, 2, "both chunks should be affected");

        // Query after consolidation - scores should be decayed
        let chunks_after = actor
            .handle_query_request("important".to_string(), 10, CausalId::new())
            .await
            .expect("query failed");
        assert_eq!(chunks_after.len(), 2, "chunks should still be found");
        assert_eq!(chunks_after[0].score, 0.9, "importance should decay to 0.9");

        // Run multiple consolidations to trigger pruning
        for _ in 0..15 {
            actor
                .backend
                .consolidate()
                .await
                .expect("consolidation failed");
        }

        // Query after heavy consolidation - chunks should be pruned
        let chunks_pruned = actor
            .handle_query_request("important".to_string(), 10, CausalId::new())
            .await
            .expect("query failed");
        assert_eq!(
            chunks_pruned.len(),
            0,
            "low-importance chunks should be pruned"
        );
    }

    #[tokio::test]
    async fn memory_actor_responds_to_user_memory_transparency_query() {
        use tokio::time::{Duration, timeout};

        let backend = Box::new(StubBackend::new());
        let mut actor = MemoryActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        let actor_handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        tokio::time::sleep(Duration::from_millis(25)).await;

        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            TransparencyQuery::UserMemory,
        )))
        .await
        .expect("transparency query broadcast should succeed");

        let mut responded = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Transparency(TransparencyEvent::QueryResponse {
                query: TransparencyQuery::UserMemory,
                result,
            }))) = timeout(Duration::from_millis(100), rx.recv()).await
            {
                assert!(
                    matches!(&*result, TransparencyResult::Memory(r) if r.memory_chunks.is_empty())
                );
                responded = true;
                break;
            }
        }

        assert!(
            responded,
            "memory actor should answer transparency user-memory queries"
        );

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast failed");
        let _ = timeout(Duration::from_secs(1), actor_handle).await;
    }

    #[tokio::test]
    async fn memory_actor_handles_context_query_through_backend() {
        let backend = Box::new(StubBackend::new());
        let actor = MemoryActor::new(backend);
        let causal_id = CausalId::new();

        let request = bus::events::memory::ContextMemoryQueryRequest {
            context_description: "coding in Rust".to_string(),
            max_chunks: 10,
            causal_id,
        };

        let result = actor.handle_context_query_request(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(
            response.chunks.len(),
            0,
            "stub backend returns empty chunks"
        );
        assert_eq!(
            response.relevance_score, 0.0,
            "empty result should have 0.0 relevance"
        );
        assert_eq!(response.causal_id, causal_id);
    }

    #[tokio::test]
    async fn context_query_calculates_aggregate_relevance_score() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let backend = persistent_backend(&temp_dir);
        let mut actor = MemoryActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        // Ingest test memories with different implicit relevance
        actor
            .handle_ingest_request(
                "Rust programming is powerful".to_string(),
                MemoryKind::Semantic,
                CausalId::new(),
            )
            .await
            .expect("ingest failed");

        actor
            .handle_ingest_request(
                "Writing Rust code for system tools".to_string(),
                MemoryKind::Episodic,
                CausalId::new(),
            )
            .await
            .expect("ingest failed");

        let causal_id = CausalId::new();
        let request = bus::events::memory::ContextMemoryQueryRequest {
            context_description: "coding in Rust".to_string(),
            max_chunks: 5,
            causal_id,
        };

        let result = actor.handle_context_query_request(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.chunks.len() > 0, "should find relevant chunks");
        assert!(
            response.relevance_score > 0.0 && response.relevance_score <= 1.0,
            "relevance_score should be in (0.0, 1.0]: got {}",
            response.relevance_score
        );
        assert_eq!(response.causal_id, causal_id);
    }

    #[tokio::test]
    async fn memory_write_request_persists_user_and_assistant_exchange() {
        use tokio::time::{Duration, timeout};

        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut actor = MemoryActor::new(persistent_backend(&temp_dir));
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        let actor_handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        tokio::time::sleep(Duration::from_millis(25)).await;

        let causal_id = CausalId::new();
        bus.broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
            text: "remember this exchange".to_string(),
            confidence: 0.99,
            causal_id,
        }))
        .await
        .expect("speech event should broadcast");

        bus.broadcast(Event::Memory(MemoryEvent::MemoryWriteRequest {
            text: "I will remember it".to_string(),
            kind: MemoryKind::Episodic,
            causal_id,
        }))
        .await
        .expect("memory write should broadcast");

        let mut completed = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Memory(MemoryEvent::MemoryWriteCompleted {
                causal_id: completed_id,
            }))) = timeout(Duration::from_millis(100), rx.recv()).await
            {
                assert_eq!(completed_id, causal_id);
                completed = true;
                break;
            }
        }
        assert!(completed, "memory write should complete");

        bus.broadcast(Event::Memory(MemoryEvent::QueryRequested {
            query: "remember exchange".to_string(),
            limit: 5,
            causal_id,
        }))
        .await
        .expect("query should broadcast");

        let mut query_saw_exchange = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::Memory(MemoryEvent::QueryCompleted { chunks, .. }))) =
                timeout(Duration::from_millis(100), rx.recv()).await
            {
                query_saw_exchange = chunks.iter().any(|chunk| {
                    chunk.content.contains("User: remember this exchange")
                        && chunk.content.contains("Assistant: I will remember it")
                });
                if query_saw_exchange {
                    break;
                }
            }
        }
        assert!(query_saw_exchange, "persisted exchange should be queryable");

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast failed");
        let _ = timeout(Duration::from_secs(1), actor_handle).await;
    }

    // ── SoulPersonalityCache tests ────────────────────────────────────────────

    #[test]
    fn soul_cache_starts_empty() {
        let cache = SoulPersonalityCache::default();
        let summary = cache.to_soul_summary();
        assert_eq!(summary.inference_cycle_count, 0);
        assert!(summary.work_patterns.is_empty());
        assert!(summary.tool_preferences.is_empty());
        assert!(summary.interest_clusters.is_empty());
    }

    #[test]
    fn soul_cache_identity_signal_frequent_app_updates_tool_preferences() {
        let mut cache = SoulPersonalityCache::default();
        let signal = DistilledIdentitySignal {
            signal_key: "frequent_app".to_string(),
            signal_value: "vscode".to_string(),
            confidence: 0.9,
        };
        cache.update_from_identity_signal(&signal);
        let summary = cache.to_soul_summary();
        assert_eq!(summary.tool_preferences, vec!["vscode".to_string()]);
        assert!(summary.interest_clusters.is_empty());
        assert!(summary.work_patterns.is_empty());
    }

    #[test]
    fn soul_cache_identity_signal_tool_pref_prefix_strips_and_updates_tool_preferences() {
        let mut cache = SoulPersonalityCache::default();
        let signal = DistilledIdentitySignal {
            signal_key: "tool_pref::cargo".to_string(),
            signal_value: "100".to_string(),
            confidence: 0.8,
        };
        cache.update_from_identity_signal(&signal);
        let summary = cache.to_soul_summary();
        assert_eq!(summary.tool_preferences, vec!["cargo".to_string()]);
    }

    #[test]
    fn soul_cache_identity_signal_interest_updates_interest_clusters() {
        let mut cache = SoulPersonalityCache::default();
        let signal = DistilledIdentitySignal {
            signal_key: "interest::rust".to_string(),
            signal_value: "rust".to_string(),
            confidence: 0.85,
        };
        cache.update_from_identity_signal(&signal);
        let summary = cache.to_soul_summary();
        assert_eq!(summary.interest_clusters, vec!["rust".to_string()]);
        assert!(summary.tool_preferences.is_empty());
    }

    #[test]
    fn soul_cache_identity_signal_work_pattern_prefix_strips_and_updates_work_patterns() {
        let mut cache = SoulPersonalityCache::default();
        let signal = DistilledIdentitySignal {
            signal_key: "work_pattern::high_cadence".to_string(),
            signal_value: "high_cadence".to_string(),
            confidence: 0.7,
        };
        cache.update_from_identity_signal(&signal);
        let summary = cache.to_soul_summary();
        assert_eq!(summary.work_patterns, vec!["high_cadence".to_string()]);
    }

    #[test]
    fn soul_cache_temporal_pattern_above_threshold_updates_work_patterns() {
        let mut cache = SoulPersonalityCache::default();
        let pattern = TemporalBehaviorPattern {
            pattern_type: "morning_coder".to_string(),
            strength: 0.8,
            first_seen: std::time::SystemTime::now(),
            last_seen: std::time::SystemTime::now(),
        };
        cache.update_from_temporal_pattern(&pattern);
        let summary = cache.to_soul_summary();
        assert_eq!(summary.work_patterns, vec!["morning_coder".to_string()]);
    }

    #[test]
    fn soul_cache_temporal_pattern_below_threshold_is_ignored() {
        let mut cache = SoulPersonalityCache::default();
        let pattern = TemporalBehaviorPattern {
            pattern_type: "weak_signal".to_string(),
            strength: 0.1, // Below TEMPORAL_PATTERN_STRENGTH_THRESHOLD (0.3)
            first_seen: std::time::SystemTime::now(),
            last_seen: std::time::SystemTime::now(),
        };
        cache.update_from_temporal_pattern(&pattern);
        let summary = cache.to_soul_summary();
        assert!(
            summary.work_patterns.is_empty(),
            "weak patterns should not appear in work_patterns"
        );
    }

    #[test]
    fn soul_cache_deduplication_prevents_duplicate_entries() {
        let mut cache = SoulPersonalityCache::default();
        for _ in 0..3 {
            let signal = DistilledIdentitySignal {
                signal_key: "frequent_app".to_string(),
                signal_value: "vscode".to_string(),
                confidence: 0.9,
            };
            cache.update_from_identity_signal(&signal);
        }
        let summary = cache.to_soul_summary();
        assert_eq!(
            summary.tool_preferences.len(),
            1,
            "duplicate values should not be added"
        );
    }

    #[test]
    fn soul_cache_respects_max_items_cap() {
        let mut cache = SoulPersonalityCache::default();
        for i in 0..(TRANSPARENCY_SUMMARY_MAX_ITEMS + 5) {
            let signal = DistilledIdentitySignal {
                signal_key: "frequent_app".to_string(),
                signal_value: format!("app_{i}"),
                confidence: 0.9,
            };
            cache.update_from_identity_signal(&signal);
        }
        let summary = cache.to_soul_summary();
        assert_eq!(
            summary.tool_preferences.len(),
            TRANSPARENCY_SUMMARY_MAX_ITEMS,
            "tool_preferences should be capped at TRANSPARENCY_SUMMARY_MAX_ITEMS"
        );
    }

    #[test]
    fn soul_cache_event_log_count_increments_correctly() {
        let mut actor = MemoryActor::new(Box::new(StubBackend::new()));
        assert_eq!(actor.soul_cache.event_log_count, 0);
        actor.soul_cache.event_log_count = actor.soul_cache.event_log_count.saturating_add(1);
        actor.soul_cache.event_log_count = actor.soul_cache.event_log_count.saturating_add(1);
        let summary = actor.soul_cache.to_soul_summary();
        assert_eq!(summary.inference_cycle_count, 2);
    }

    #[test]
    fn soul_cache_event_log_count_does_not_overflow() {
        let mut cache = SoulPersonalityCache {
            event_log_count: usize::MAX,
            ..Default::default()
        };
        cache.event_log_count = cache.event_log_count.saturating_add(1);
        assert_eq!(cache.event_log_count, usize::MAX, "should saturate at max");
    }

    #[tokio::test]
    async fn memory_actor_soul_cache_populated_via_bus_events() {
        use tokio::time::{Duration, sleep};

        let backend = Box::new(StubBackend::new());
        let mut actor = MemoryActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        let handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        sleep(Duration::from_millis(25)).await;

        // Emit a SoulEvent::EventLogged to update event_log_count
        bus.broadcast(Event::Soul(SoulEvent::EventLogged {
            row_id: 1,
            causal_id: CausalId::new(),
        }))
        .await
        .expect("broadcast failed");

        // Emit a SoulEvent::IdentitySignalDistilled
        bus.broadcast(Event::Soul(SoulEvent::IdentitySignalDistilled {
            signal: DistilledIdentitySignal {
                signal_key: "frequent_app".to_string(),
                signal_value: "cargo".to_string(),
                confidence: 0.9,
            },
            causal_id: CausalId::new(),
        }))
        .await
        .expect("broadcast failed");

        // Emit a SoulEvent::TemporalPatternDetected
        bus.broadcast(Event::Soul(SoulEvent::TemporalPatternDetected {
            pattern: TemporalBehaviorPattern {
                pattern_type: "late_night_writer".to_string(),
                strength: 0.7,
                first_seen: std::time::SystemTime::now(),
                last_seen: std::time::SystemTime::now(),
            },
            causal_id: CausalId::new(),
        }))
        .await
        .expect("broadcast failed");

        // Wait for soul events to be processed, then issue a UserMemory query
        sleep(Duration::from_millis(50)).await;

        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            TransparencyQuery::UserMemory,
        )))
        .await
        .expect("broadcast failed");

        // Look for a MemoryResponse with the real soul data
        let mut got_soul_data = false;
        for _ in 0..30 {
            if let Ok(Ok(Event::Transparency(TransparencyEvent::QueryResponse {
                query: TransparencyQuery::UserMemory,
                result,
            }))) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                if let TransparencyResult::Memory(mem_response) = &*result {
                    // inference_cycle_count should be 1 (one EventLogged event)
                    if mem_response.soul_summary.inference_cycle_count == 1
                        && mem_response
                            .soul_summary
                            .tool_preferences
                            .contains(&"cargo".to_string())
                        && mem_response
                            .soul_summary
                            .work_patterns
                            .contains(&"late_night_writer".to_string())
                    {
                        got_soul_data = true;
                        break;
                    }
                }
            }
        }

        assert!(
            got_soul_data,
            "memory actor should populate soul summary from bus events"
        );

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast failed");
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }
}
