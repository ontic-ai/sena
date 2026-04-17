//! SoulActor: owns the encrypted store and handles all Soul subsystem events.

use std::path::PathBuf;
use std::sync::Arc;

use bus::events::{PersonalityMetadata, WorkCadence as BusWorkCadence};
use bus::{Actor, ActorError, Event, EventBus, SoulEvent};
use tokio::sync::{broadcast, mpsc};

use crate::error::SoulError;
use crate::schema::{SchemaV1, WorkCadence};
use crate::store::SoulStore;

const SOUL_ACTOR_NAME: &str = "soul";
const SOUL_CHANNEL_CAPACITY: usize = 256;

/// Actor that owns the Soul encrypted store.
///
/// Responsibilities:
/// - Handle SoulEvent::WriteRequested and persist to store
/// - Handle SoulEvent::SummaryRequested and read from store
/// - Handle SoulEvent::ExportRequested and write JSON export
/// - Handle SoulEvent::DeleteRequested/DeleteConfirmed and wipe store
/// - Manage identity signals via store abstraction
/// - Track temporal patterns
/// - Emit PersonalityUpdated after boot and schema updates
pub struct SoulActor {
    /// Encrypted store abstraction.
    store: Option<Box<dyn SoulStore>>,
    /// Event bus reference.
    bus: Option<Arc<EventBus>>,
    /// Broadcast receiver for all bus events.
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    /// Directed mpsc receiver for Soul-specific events.
    directed_rx: Option<mpsc::Receiver<Event>>,
    /// In-memory schema cache.
    schema: SchemaV1,
    /// Deletion pending confirmation?
    deletion_pending: bool,
}

impl SoulActor {
    /// Create a new SoulActor with the given store implementation.
    pub fn new(store: Box<dyn SoulStore>) -> Self {
        Self {
            store: Some(store),
            bus: None,
            broadcast_rx: None,
            directed_rx: None,
            schema: SchemaV1::default(),
            deletion_pending: false,
        }
    }

    /// Emit PersonalityUpdated event.
    async fn emit_personality_updated(&self, causal_id: bus::CausalId) {
        if let Some(bus) = &self.bus {
            let metadata = PersonalityMetadata {
                verbosity: self.schema.verbosity_preference,
                warmth: self.schema.response_warmth,
                work_cadence: match self.schema.work_cadence_preference {
                    WorkCadence::Burst => BusWorkCadence::Burst,
                    WorkCadence::Steady => BusWorkCadence::Steady,
                    WorkCadence::LongFocus => BusWorkCadence::LongFocus,
                },
            };

            let _ = bus
                .broadcast(Event::Soul(SoulEvent::PersonalityUpdated {
                    metadata,
                    causal_id,
                }))
                .await;
        }
    }

    /// Emit existing identity signals and temporal patterns from store to CTP.
    /// Called once during startup after store initialization.
    async fn emit_existing_signals(&self) {
        let store = match &self.store {
            Some(s) => s,
            None => return,
        };

        let bus = match &self.bus {
            Some(b) => b,
            None => return,
        };

        let causal_id = bus::CausalId::new();

        // Emit identity signals
        if let Ok(signals) = store.read_all_identity_signals() {
            for signal in signals {
                let distilled = bus::events::soul::DistilledIdentitySignal {
                    signal_key: signal.key,
                    signal_value: signal.value,
                    confidence: 1.0, // Stored signals have high confidence
                };

                let _ = bus
                    .broadcast(Event::Soul(SoulEvent::IdentitySignalDistilled {
                        signal: distilled,
                        causal_id,
                    }))
                    .await;
            }
        }

        // Emit temporal patterns
        if let Ok(patterns) = store.read_temporal_patterns() {
            for pattern in patterns {
                let behavior_pattern = bus::events::soul::TemporalBehaviorPattern {
                    pattern_type: pattern.pattern_type,
                    strength: pattern.strength,
                    first_seen: pattern.first_seen,
                    last_seen: pattern.last_seen,
                };

                let _ = bus
                    .broadcast(Event::Soul(SoulEvent::TemporalPatternDetected {
                        pattern: behavior_pattern,
                        causal_id,
                    }))
                    .await;
            }
        }
    }

    /// Handle SoulEvent::WriteRequested.
    async fn handle_write_requested(
        &mut self,
        description: String,
        app_context: Option<String>,
        timestamp: std::time::SystemTime,
        causal_id: bus::CausalId,
    ) -> Result<(), SoulError> {
        tracing::debug!(
            ?causal_id,
            description_len = description.len(),
            ?app_context,
            "SoulActor: WriteRequested received"
        );

        let store = self.store.as_mut().ok_or(SoulError::StoreNotInitialized)?;

        let row_id = store.write_event(description, app_context, timestamp)?;

        tracing::info!(row_id, ?causal_id, "SoulActor: event written to store");

        if let Some(bus) = &self.bus {
            let _ = bus
                .broadcast(Event::Soul(SoulEvent::EventLogged { row_id, causal_id }))
                .await;
        }

        // Emit PersonalityUpdated after write (schema may have changed).
        self.emit_personality_updated(causal_id).await;

        Ok(())
    }

    /// Handle SoulEvent::SummaryRequested.
    async fn handle_summary_requested(
        &mut self,
        max_events: usize,
        causal_id: bus::CausalId,
    ) -> Result<(), SoulError> {
        tracing::debug!(
            ?causal_id,
            max_events,
            "SoulActor: SummaryRequested received"
        );

        let store = self.store.as_ref().ok_or(SoulError::StoreNotInitialized)?;

        let summary = store.read_summary(max_events, None)?;

        tracing::info!(
            event_count = summary.event_count,
            content_len = summary.content.len(),
            ?causal_id,
            "SoulActor: summary read from store"
        );

        if let Some(bus) = &self.bus {
            let _ = bus
                .broadcast(Event::Soul(SoulEvent::SummaryCompleted {
                    content: summary.content,
                    event_count: summary.event_count,
                    causal_id,
                }))
                .await;
        }

        Ok(())
    }

    /// Handle SoulEvent::ExportRequested.
    async fn handle_export_requested(
        &self,
        path: PathBuf,
        causal_id: bus::CausalId,
    ) -> Result<(), SoulError> {
        tracing::info!(?path, ?causal_id, "SoulActor: ExportRequested received");

        let store = self.store.as_ref().ok_or(SoulError::StoreNotInitialized)?;

        // Build export structure.
        let summary = store.read_summary(usize::MAX, None)?;
        let signals = store.read_all_identity_signals()?;
        let patterns = store.read_temporal_patterns()?;

        let export = serde_json::json!({
            "schema": self.schema,
            "summary": summary,
            "identity_signals": signals,
            "temporal_patterns": patterns,
        });

        // Write to file.
        let json_str = serde_json::to_string_pretty(&export)
            .map_err(|e| SoulError::InvalidInput(format!("JSON serialization: {}", e)))?;
        std::fs::write(&path, json_str)?;

        tracing::info!(?path, "SoulActor: export completed");

        if let Some(bus) = &self.bus {
            let _ = bus
                .broadcast(Event::Soul(SoulEvent::ExportCompleted { path, causal_id }))
                .await;
        }

        Ok(())
    }

    /// Handle SoulEvent::DeleteRequested.
    async fn handle_delete_requested(&mut self, causal_id: bus::CausalId) -> Result<(), SoulError> {
        tracing::warn!(
            ?causal_id,
            "SoulActor: DeleteRequested received — awaiting confirmation"
        );

        self.deletion_pending = true;

        // Deletion requires explicit confirmation via DeleteConfirmed event.
        // This is intentionally non-automatic to prevent accidental data loss.

        Ok(())
    }

    /// Handle SoulEvent::DeleteConfirmed.
    async fn handle_delete_confirmed(&mut self, causal_id: bus::CausalId) -> Result<(), SoulError> {
        if !self.deletion_pending {
            tracing::warn!(
                ?causal_id,
                "SoulActor: DeleteConfirmed received without prior DeleteRequested — ignoring"
            );
            return Ok(());
        }

        tracing::warn!(
            ?causal_id,
            "SoulActor: DeleteConfirmed received — wiping all Soul data"
        );

        // Wipe the store (closes, deletes file, re-initializes).
        if let Some(store) = &mut self.store {
            store.wipe()?;
        }

        // Reset actor schema to match wiped store.
        self.schema = SchemaV1::default();
        self.deletion_pending = false;

        tracing::warn!("SoulActor: all Soul data deleted");

        if let Some(bus) = &self.bus {
            let _ = bus
                .broadcast(Event::Soul(SoulEvent::Deleted { causal_id }))
                .await;
        }

        Ok(())
    }

    /// Process a single event from the bus.
    async fn process_event(&mut self, event: Event) -> Result<(), SoulError> {
        match event {
            Event::Soul(soul_event) => match soul_event {
                SoulEvent::WriteRequested {
                    description,
                    app_context,
                    timestamp,
                    causal_id,
                } => {
                    if let Err(e) = self
                        .handle_write_requested(description, app_context, timestamp, causal_id)
                        .await
                    {
                        tracing::error!(error = %e, "SoulActor: WriteRequested handler failed");
                        if let Some(bus) = &self.bus {
                            let _ = bus
                                .broadcast(Event::Soul(SoulEvent::OperationFailed {
                                    reason: e.to_string(),
                                    causal_id,
                                }))
                                .await;
                        }
                    }
                }
                SoulEvent::SummaryRequested {
                    max_events,
                    causal_id,
                } => {
                    if let Err(e) = self.handle_summary_requested(max_events, causal_id).await {
                        tracing::error!(error = %e, "SoulActor: SummaryRequested handler failed");
                        if let Some(bus) = &self.bus {
                            let _ = bus
                                .broadcast(Event::Soul(SoulEvent::OperationFailed {
                                    reason: e.to_string(),
                                    causal_id,
                                }))
                                .await;
                        }
                    }
                }
                SoulEvent::ExportRequested { path, causal_id } => {
                    if let Err(e) = self.handle_export_requested(path.clone(), causal_id).await {
                        tracing::error!(error = %e, "SoulActor: ExportRequested handler failed");
                        if let Some(bus) = &self.bus {
                            let _ = bus
                                .broadcast(Event::Soul(SoulEvent::ExportFailed {
                                    reason: e.to_string(),
                                    causal_id,
                                }))
                                .await;
                        }
                    }
                }
                SoulEvent::DeleteRequested { causal_id } => {
                    if let Err(e) = self.handle_delete_requested(causal_id).await {
                        tracing::error!(error = %e, "SoulActor: DeleteRequested handler failed");
                    }
                }
                SoulEvent::DeleteConfirmed { causal_id } => {
                    if let Err(e) = self.handle_delete_confirmed(causal_id).await {
                        tracing::error!(error = %e, "SoulActor: DeleteConfirmed handler failed");
                        if let Some(bus) = &self.bus {
                            let _ = bus
                                .broadcast(Event::Soul(SoulEvent::OperationFailed {
                                    reason: e.to_string(),
                                    causal_id,
                                }))
                                .await;
                        }
                    }
                }
                SoulEvent::EventLogged { .. }
                | SoulEvent::SummaryCompleted { .. }
                | SoulEvent::OperationFailed { .. }
                | SoulEvent::PersonalityUpdated { .. }
                | SoulEvent::ExportCompleted { .. }
                | SoulEvent::ExportFailed { .. }
                | SoulEvent::Deleted { .. }
                | SoulEvent::IdentitySignalDistilled { .. }
                | SoulEvent::TemporalPatternDetected { .. } => {
                    // These are emitted by SoulActor itself; ignore to prevent loops.
                    tracing::trace!("SoulActor: ignoring self-emitted event");
                }
            },
            Event::System(bus::SystemEvent::ShutdownSignal) => {
                tracing::info!("SoulActor: shutdown signal received");
                return Err(SoulError::NotImplemented(
                    "shutdown signal stop".to_string(),
                ));
            }
            _ => {
                // Ignore other event types.
                tracing::trace!("SoulActor: ignoring non-soul event");
            }
        }
        Ok(())
    }
}

impl Actor for SoulActor {
    fn name(&self) -> &'static str {
        SOUL_ACTOR_NAME
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        tracing::info!("SoulActor: starting");

        // Initialize store.
        if let Some(store) = &mut self.store {
            store
                .initialize()
                .map_err(|e| ActorError::StartupFailed(e.to_string()))?;
        } else {
            return Err(ActorError::StartupFailed(
                "store not configured".to_string(),
            ));
        }

        // Subscribe to bus channels.
        let broadcast_rx = bus.subscribe_broadcast();
        let (directed_tx, directed_rx) = mpsc::channel(SOUL_CHANNEL_CAPACITY);
        bus.register_directed(SOUL_ACTOR_NAME, directed_tx)
            .map_err(|e| ActorError::StartupFailed(e.to_string()))?;

        self.bus = Some(bus.clone());
        self.broadcast_rx = Some(broadcast_rx);
        self.directed_rx = Some(directed_rx);

        // Emit PersonalityUpdated at boot completion.
        self.emit_personality_updated(bus::CausalId::new()).await;

        // Emit existing identity signals and temporal patterns to CTP.
        self.emit_existing_signals().await;

        tracing::info!("SoulActor: started successfully");
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        tracing::info!("SoulActor: entering main event loop");

        let mut broadcast_rx = self
            .broadcast_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("broadcast_rx not initialized".to_string()))?;
        let mut directed_rx = self
            .directed_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("directed_rx not initialized".to_string()))?;

        loop {
            tokio::select! {
                Ok(event) = broadcast_rx.recv() => {
                    if let Err(e) = self.process_event(event).await {
                        if matches!(e, SoulError::NotImplemented(_)) {
                            tracing::info!("SoulActor: exiting event loop on shutdown signal");
                            break;
                        }
                        tracing::error!(error = %e, "SoulActor: error processing broadcast event");
                    }
                }
                Some(event) = directed_rx.recv() => {
                    if let Err(e) = self.process_event(event).await {
                        if matches!(e, SoulError::NotImplemented(_)) {
                            tracing::info!("SoulActor: exiting event loop on shutdown signal");
                            break;
                        }
                        tracing::error!(error = %e, "SoulActor: error processing directed event");
                    }
                }
                else => {
                    tracing::warn!("SoulActor: all channels closed, exiting event loop");
                    break;
                }
            }
        }

        tracing::info!("SoulActor: event loop exited");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        tracing::info!("SoulActor: stopping");

        if let Some(store) = &mut self.store {
            store
                .close()
                .map_err(|e| ActorError::RuntimeError(e.to_string()))?;
        }

        tracing::info!("SoulActor: stopped successfully");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IdentitySignal, SoulEventRecord, SoulSummary, TemporalPattern};
    use std::time::SystemTime;

    /// Stub store for testing.
    struct TestStore {
        events: Vec<SoulEventRecord>,
        signals: Vec<IdentitySignal>,
    }

    impl TestStore {
        fn new() -> Self {
            Self {
                events: Vec::new(),
                signals: Vec::new(),
            }
        }
    }

    impl SoulStore for TestStore {
        fn write_event(
            &mut self,
            description: String,
            app_context: Option<String>,
            timestamp: SystemTime,
        ) -> Result<u64, SoulError> {
            let row_id = self.events.len() as u64;
            self.events.push(SoulEventRecord {
                row_id,
                description,
                app_context,
                timestamp,
            });
            Ok(row_id)
        }

        fn read_summary(
            &self,
            max_events: usize,
            _max_chars: Option<usize>,
        ) -> Result<SoulSummary, SoulError> {
            let count = self.events.len().min(max_events);
            let content = format!("{} events", count);
            Ok(SoulSummary {
                content,
                event_count: count,
            })
        }

        fn read_event(&self, row_id: u64) -> Result<Option<SoulEventRecord>, SoulError> {
            Ok(self.events.get(row_id as usize).cloned())
        }

        fn write_identity_signal(&mut self, key: &str, value: &str) -> Result<(), SoulError> {
            self.signals.push(IdentitySignal {
                key: key.to_string(),
                value: value.to_string(),
            });
            Ok(())
        }

        fn read_identity_signal(&self, key: &str) -> Result<Option<String>, SoulError> {
            Ok(self
                .signals
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.value.clone()))
        }

        fn read_all_identity_signals(&self) -> Result<Vec<IdentitySignal>, SoulError> {
            Ok(self.signals.clone())
        }

        fn increment_identity_counter(&mut self, key: &str, delta: u64) -> Result<(), SoulError> {
            let current = self
                .read_identity_signal(key)?
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let next = current.saturating_add(delta);
            self.write_identity_signal(key, &next.to_string())
        }

        fn write_temporal_pattern(&mut self, _pattern: TemporalPattern) -> Result<(), SoulError> {
            Ok(())
        }

        fn read_temporal_patterns(&self) -> Result<Vec<TemporalPattern>, SoulError> {
            Ok(Vec::new())
        }

        fn initialize(&mut self) -> Result<(), SoulError> {
            Ok(())
        }

        fn close(&mut self) -> Result<(), SoulError> {
            Ok(())
        }

        fn wipe(&mut self) -> Result<(), SoulError> {
            Ok(())
        }
    }

    #[test]
    fn soul_actor_constructs_with_store() {
        let store = Box::new(TestStore::new());
        let actor = SoulActor::new(store);
        assert_eq!(actor.name(), "soul");
    }

    #[tokio::test]
    async fn soul_actor_lifecycle_completes() {
        let store = Box::new(TestStore::new());
        let mut actor = SoulActor::new(store);
        let bus = Arc::new(EventBus::new());

        actor.start(Arc::clone(&bus)).await.expect("start failed");
        actor.stop().await.expect("stop failed");
    }

    #[tokio::test]
    async fn soul_actor_emits_identity_signals_on_startup() {
        // Arrange: Create a store with existing identity signals
        let mut store = Box::new(TestStore::new());
        store.signals = vec![
            IdentitySignal {
                key: "voice::rate".to_string(),
                value: "1.2".to_string(),
            },
            IdentitySignal {
                key: "work_style::cadence".to_string(),
                value: "burst".to_string(),
            },
        ];

        let mut actor = SoulActor::new(store);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        // Act: Start the actor
        actor.start(Arc::clone(&bus)).await.expect("start failed");

        // Assert: Verify PersonalityUpdated and IdentitySignalDistilled events emitted
        let mut identity_signals_received = 0;
        let mut personality_updated_received = false;

        for _ in 0..10 {
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(Event::Soul(SoulEvent::PersonalityUpdated { .. }))) => {
                    personality_updated_received = true;
                }
                Ok(Ok(Event::Soul(SoulEvent::IdentitySignalDistilled { signal, .. }))) => {
                    identity_signals_received += 1;
                    assert!(
                        signal.signal_key == "voice::rate"
                            || signal.signal_key == "work_style::cadence"
                    );
                    assert_eq!(signal.confidence, 1.0);
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => break,
            }
        }

        assert!(
            personality_updated_received,
            "PersonalityUpdated not received"
        );
        assert_eq!(
            identity_signals_received, 2,
            "Expected 2 IdentitySignalDistilled events"
        );

        actor.stop().await.expect("stop failed");
    }
}
