//! Event bus implementation.

use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::{broadcast, mpsc};

use crate::causal::CausalId;
use crate::events::{
    CTPEvent, InferenceEvent, MemoryEvent, ModelEvent, PlatformEvent, SoulEvent, SpeechEvent,
    SystemEvent, TelemetryEvent,
};

/// Unified event type for all bus communication.
#[derive(Debug, Clone)]
pub enum Event {
    /// System-level events (boot, shutdown, failures).
    System(SystemEvent),
    /// Platform-layer events (window, clipboard, file, keystroke).
    Platform(PlatformEvent),
    /// CTP (Continuous Thought Processing) events.
    CTP(Box<CTPEvent>),
    /// Inference-layer events (model discovery, inference requests/responses).
    Inference(InferenceEvent),
    /// Memory subsystem events (write/query requests and responses).
    Memory(MemoryEvent),
    /// Soul subsystem events (event log writes and summaries).
    Soul(SoulEvent),
    /// Speech subsystem events (STT/TTS).
    Speech(SpeechEvent),
    /// Model management events.
    Model(ModelEvent),
    /// Telemetry and metrics events.
    Telemetry(TelemetryEvent),
}

impl Event {
    /// Extract causal_id from the event if it has one.
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Event::Inference(e) => e.causal_id(),
            Event::Memory(e) => e.causal_id(),
            Event::Speech(e) => e.causal_id(),
            Event::Soul(e) => e.causal_id(),
            _ => None,
        }
    }
}

/// Bus operation errors.
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    /// Channel closed during send operation.
    #[error("channel closed: {0}")]
    ChannelClosed(String),

    /// Directed send to actor that does not exist in registry.
    #[error("actor not found: {0}")]
    ActorNotFound(String),

    /// Internal lock was poisoned (indicates a prior panic).
    #[error("registry lock poisoned")]
    LockPoisoned,
}

/// Event bus managing broadcast and directed channels.
///
/// The bus has two routing modes:
/// - Broadcast: one-to-many for system events (all actors receive)
/// - Directed: one-to-one for targeted actor communication
pub struct EventBus {
    /// Broadcast sender for one-to-many system events.
    broadcast_tx: broadcast::Sender<Event>,
    /// Registry of directed mpsc senders, keyed by actor name.
    mpsc_registry: RwLock<HashMap<&'static str, mpsc::Sender<Event>>>,
}

impl EventBus {
    /// Create a new event bus with default broadcast channel capacity (1024).
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            broadcast_tx,
            mpsc_registry: RwLock::new(HashMap::new()),
        }
    }

    /// Subscribe to broadcast channel. Returns a new receiver.
    pub fn subscribe_broadcast(&self) -> broadcast::Receiver<Event> {
        self.broadcast_tx.subscribe()
    }

    /// Broadcast an event to all subscribers.
    ///
    /// Logs the emission with event type and optional causal_id.
    pub async fn broadcast(&self, event: Event) -> Result<(), BusError> {
        let causal_id = event.causal_id();
        let event_type = match &event {
            Event::System(e) => format!("System::{:?}", e).split('(').next().unwrap_or("System").to_string(),
            Event::Platform(e) => format!("Platform::{:?}", e).split('(').next().unwrap_or("Platform").to_string(),
            Event::CTP(e) => format!("CTP::{:?}", e).split('(').next().unwrap_or("CTP").to_string(),
            Event::Inference(e) => format!("Inference::{:?}", e).split('(').next().unwrap_or("Inference").to_string(),
            Event::Memory(e) => format!("Memory::{:?}", e).split('(').next().unwrap_or("Memory").to_string(),
            Event::Soul(e) => format!("Soul::{:?}", e).split('(').next().unwrap_or("Soul").to_string(),
            Event::Speech(e) => format!("Speech::{:?}", e).split('(').next().unwrap_or("Speech").to_string(),
            Event::Model(e) => format!("Model::{:?}", e).split('(').next().unwrap_or("Model").to_string(),
            Event::Telemetry(e) => format!("Telemetry::{:?}", e).split('(').next().unwrap_or("Telemetry").to_string(),
        };

        if let Some(cid) = causal_id {
            tracing::trace!("BUS EMIT {} causal_id={}", event_type, cid.as_u64());
        } else {
            tracing::trace!("BUS EMIT {}", event_type);
        }

        self.broadcast_tx
            .send(event)
            .map(|_| ())
            .map_err(|e| BusError::ChannelClosed(format!("broadcast send failed: {}", e)))
    }

    /// Register a directed mpsc sender for a named actor.
    pub fn register_directed(
        &self,
        name: &'static str,
        tx: mpsc::Sender<Event>,
    ) -> Result<(), BusError> {
        self.mpsc_registry
            .write()
            .map_err(|_| BusError::LockPoisoned)?
            .insert(name, tx);
        Ok(())
    }

    /// Send an event to a specific named actor via directed channel.
    pub async fn send_directed(&self, name: &'static str, event: Event) -> Result<(), BusError> {
        let tx = self
            .mpsc_registry
            .read()
            .map_err(|_| BusError::LockPoisoned)?
            .get(name)
            .cloned()
            .ok_or_else(|| BusError::ActorNotFound(name.to_string()))?;

        tx.send(event).await.map_err(|e| {
            BusError::ChannelClosed(format!("directed send to {} failed: {}", name, e))
        })
    }

    /// Get the number of broadcast subscribers (for testing).
    #[cfg(test)]
    pub fn subscriber_count(&self) -> usize {
        self.broadcast_tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::system::SystemEvent;

    #[tokio::test]
    async fn broadcast_delivers_to_all_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe_broadcast();
        let mut rx2 = bus.subscribe_broadcast();

        let event = Event::System(SystemEvent::BootComplete);
        bus.broadcast(event.clone()).await.expect("broadcast failed");

        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[tokio::test]
    async fn broadcast_handles_closed_receiver() {
        let bus = EventBus::new();
        let rx1 = bus.subscribe_broadcast();
        let mut rx2 = bus.subscribe_broadcast();

        drop(rx1);

        let event = Event::System(SystemEvent::ShutdownSignal);
        assert!(bus.broadcast(event).await.is_ok());

        assert!(rx2.recv().await.is_ok());
    }

    #[tokio::test]
    async fn directed_send_to_registered_actor() {
        let bus = EventBus::new();
        let (tx, mut rx) = mpsc::channel(16);

        bus.register_directed("test_actor", tx).expect("registration failed");

        let event = Event::System(SystemEvent::BootComplete);
        bus.send_directed("test_actor", event)
            .await
            .expect("directed send failed");

        assert!(rx.recv().await.is_some());
    }

    #[tokio::test]
    async fn directed_send_to_missing_actor_fails() {
        let bus = EventBus::new();
        let event = Event::System(SystemEvent::BootComplete);

        let result = bus.send_directed("missing_actor", event).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(BusError::ActorNotFound(_))));
    }

    #[test]
    fn subscriber_count_helper_works() {
        let bus = EventBus::new();
        assert_eq!(bus.subscriber_count(), 0);

        let _rx1 = bus.subscribe_broadcast();
        assert_eq!(bus.subscriber_count(), 1);

        let _rx2 = bus.subscribe_broadcast();
        assert_eq!(bus.subscriber_count(), 2);
    }
}
