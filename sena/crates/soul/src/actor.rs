//! SoulActor: owns the encrypted store and handles all Soul subsystem events.

use std::sync::Arc;

use bus::{Actor, ActorError, Event, EventBus, SoulEvent};
use tokio::sync::{broadcast, mpsc};

use crate::error::SoulError;
use crate::store::SoulStore;

const SOUL_ACTOR_NAME: &str = "soul";
const SOUL_CHANNEL_CAPACITY: usize = 256;

/// Actor that owns the Soul encrypted store.
///
/// Responsibilities:
/// - Handle SoulEvent::WriteRequested and persist to store
/// - Handle SoulEvent::SummaryRequested and read from store
/// - Manage identity signals via store abstraction
/// - Track temporal patterns
pub struct SoulActor {
    /// Encrypted store abstraction.
    store: Option<Box<dyn SoulStore>>,
    /// Event bus reference.
    bus: Option<Arc<EventBus>>,
    /// Broadcast receiver for all bus events.
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    /// Directed mpsc receiver for Soul-specific events.
    directed_rx: Option<mpsc::Receiver<Event>>,
}

impl SoulActor {
    /// Create a new SoulActor with the given store implementation.
    pub fn new(store: Box<dyn SoulStore>) -> Self {
        Self {
            store: Some(store),
            bus: None,
            broadcast_rx: None,
            directed_rx: None,
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
                    self.handle_write_requested(description, app_context, timestamp, causal_id)
                        .await?;
                }
                SoulEvent::SummaryRequested {
                    max_events,
                    causal_id,
                } => {
                    self.handle_summary_requested(max_events, causal_id).await?;
                }
                SoulEvent::EventLogged { .. }
                | SoulEvent::SummaryCompleted { .. }
                | SoulEvent::OperationFailed { .. } => {
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

        self.bus = Some(bus);
        self.broadcast_rx = Some(broadcast_rx);
        self.directed_rx = Some(directed_rx);

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
}
