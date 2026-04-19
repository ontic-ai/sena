//! Memory actor: manages memory ingestion, consolidation, and retrieval.

use crate::backend::MemoryBackend;
use crate::error::MemoryError;
use bus::events::{MemoryEvent, MemoryKind, ScoredChunk, SystemEvent};
use bus::{Actor, ActorError, CausalId, Event, EventBus};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

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
}

impl MemoryActor {
    /// Create a new memory actor with the given backend.
    pub fn new(backend: Box<dyn MemoryBackend>) -> Self {
        Self {
            bus: None,
            backend,
            backup_config: BackupConfig::default(),
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

    /// Perform a backup of working memory state to the configured backup directory.
    ///
    /// Writes a JSON summary of metadata to a timestamped file.
    /// Returns the path of the backup file written.
    async fn perform_backup(&self, causal_id: CausalId) -> Result<PathBuf, String> {
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

        // Write a JSON summary (stub: exports metadata only, no raw content)
        // TODO(security): When ech0 backend is fully implemented, encrypt backup content
        // using the workspace's CryptoLayer before writing to disk. Current stub writes
        // minimal metadata only (no sensitive user content).
        let summary = serde_json::json!({
            "backup_format_version": 1,
            "timestamp_unix": now.as_secs(),
            "causal_id": causal_id.as_u64(),
            "note": "stub backup — full ech0 export pending store integration"
        });

        let content = serde_json::to_string_pretty(&summary)
            .map_err(|e| format!("failed to serialize backup summary: {e}"))?;

        std::fs::write(&backup_path, content)
            .map_err(|e| format!("failed to write backup file: {e}"))?;

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
                            Ok(Event::System(SystemEvent::ShutdownInitiated)) => {
                                info!("shutdown initiated — performing memory backup");
                                let causal_id = CausalId::new();
                                match self.perform_backup(causal_id).await {
                                    Ok(path) => {
                                        let _ =
                                            bus.broadcast(Event::Memory(
                                                MemoryEvent::BackupCompleted { path, causal_id },
                                            ))
                                            .await;
                                    }
                                    Err(reason) => {
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
                            Ok(Event::Memory(memory_event)) => match memory_event {
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
                                _ => {}
                            },
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
        use crate::echo0_backend::Echo0Backend;

        let backend = Box::new(Echo0Backend::new());
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
}
