//! Event bus implementation.

use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::{broadcast, mpsc};

use crate::events::{CTPEvent, PlatformEvent, SystemEvent};

/// Unified event type for all bus communication.
#[derive(Debug, Clone)]
pub enum Event {
    /// System-level events (boot, shutdown, failures).
    System(SystemEvent),
    /// Platform-layer events (window, clipboard, file, keystroke).
    Platform(PlatformEvent),
    /// CTP (Continuous Thought Processing) events.
    CTP(CTPEvent),
}

/// Bus operation errors.
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    /// Channel closed during send operation.
    #[error("channel closed: {0}")]
    ChannelClosed(String),

    /// Directed send to actor that doesn't exist in registry.
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
    pub async fn broadcast(&self, event: Event) -> Result<(), BusError> {
        self.broadcast_tx
            .send(event)
            .map(|_| ())
            .map_err(|e| BusError::ChannelClosed(format!("broadcast send failed: {}", e)))
    }

    /// Register a directed mpsc sender for a named actor.
    pub fn register_directed(&self, name: &'static str, tx: mpsc::Sender<Event>) -> Result<(), BusError> {
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
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SystemEvent;

    #[tokio::test]
    async fn broadcast_delivers_to_all_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe_broadcast();
        let mut rx2 = bus.subscribe_broadcast();

        let event = Event::System(SystemEvent::BootComplete);
        bus.broadcast(event.clone()).await.unwrap();

        // Both receivers should get the event
        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[tokio::test]
    async fn broadcast_handles_closed_receiver() {
        let bus = EventBus::new();
        let rx1 = bus.subscribe_broadcast();
        let mut rx2 = bus.subscribe_broadcast(); // Keep one alive

        drop(rx1); // Close one receiver

        // Broadcast should still succeed with at least one receiver active
        let event = Event::System(SystemEvent::ShutdownSignal);
        assert!(bus.broadcast(event).await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[tokio::test]
    async fn directed_send_delivers_to_registered_actor() {
        let bus = EventBus::new();
        let (tx, mut rx) = mpsc::channel(16);

        bus.register_directed("test_actor", tx).expect("register_directed failed");

        let event = Event::System(SystemEvent::BootComplete);
        bus.send_directed("test_actor", event).await.unwrap();

        assert!(rx.recv().await.is_some());
    }

    #[tokio::test]
    async fn directed_send_returns_error_for_unregistered_actor() {
        let bus = EventBus::new();
        let event = Event::System(SystemEvent::BootComplete);

        let result = bus.send_directed("nonexistent", event).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BusError::ActorNotFound(_)));
    }

    #[test]
    fn actor_error_variants_construct_correctly() {
        use crate::actor::ActorError;

        let err1 = ActorError::ChannelClosed("test".to_string());
        assert!(err1.to_string().contains("channel closed"));

        let err2 = ActorError::StartupFailed("test".to_string());
        assert!(err2.to_string().contains("startup failed"));

        let err3 = ActorError::RuntimeError("test".to_string());
        assert!(err3.to_string().contains("runtime error"));
    }
}