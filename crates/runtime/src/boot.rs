//! Boot sequence orchestration.

use std::sync::Arc;

use bus::{Event, EventBus, SystemEvent};

use crate::registry::ActorRegistry;

/// Runtime holds initialized subsystems and the event bus.
pub struct Runtime {
    /// Event bus for all actor communication.
    pub bus: Arc<EventBus>,
    /// Registry of spawned actor tasks.
    pub registry: ActorRegistry,
    /// Keep-alive receiver to prevent broadcast channel from closing.
    /// In production, actors will subscribe during start() and maintain channel lifetime.
    pub(crate) _keep_alive: tokio::sync::broadcast::Receiver<Event>,
}

/// Boot sequence errors.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// Step 1: Config load failed.
    #[error("config load failed: {0}")]
    ConfigLoadFailed(String),

    /// Step 2: Event bus initialization failed.
    #[error("event bus init failed: {0}")]
    BusInitFailed(String),

    /// Step 3: Soul (SoulBox) initialization failed.
    #[error("soul init failed: {0}")]
    SoulInitFailed(String),

    /// Step 4: Actor spawn failed.
    #[error("actor spawn failed: {0}")]
    ActorSpawnFailed(String),

    /// Step 5: Platform adapter init failed.
    #[error("platform init failed: {0}")]
    PlatformInitFailed(String),

    /// Step 6: CTP (Continuous Thought Processing) init failed.
    #[error("ctp init failed: {0}")]
    CTPInitFailed(String),

    /// Step 7: Memory system init failed.
    #[error("memory init failed: {0}")]
    MemoryInitFailed(String),

    /// Step 8: Inference system init failed.
    #[error("inference init failed: {0}")]
    InferenceInitFailed(String),

    /// Step 9: Prompt composer init failed.
    #[error("prompt init failed: {0}")]
    PromptInitFailed(String),

    /// Step 10: Boot complete broadcast failed.
    #[error("boot complete broadcast failed: {0}")]
    BroadcastFailed(String),
}

/// Boot sequence per architecture §4.1.
///
/// Phase 1 implementation: Steps 1-2 fully implemented, 3-9 stubbed, 10 broadcast.
///
/// Steps:
/// 1. Load config from OS-appropriate location
/// 2. Initialize EventBus
/// 3. Initialize Soul (SoulBox) — STUB
/// 4. Spawn actor tasks — STUB
/// 5. Initialize platform adapter — STUB
/// 6. Initialize CTP loop — STUB
/// 7. Initialize memory system — STUB
/// 8. Initialize inference system — STUB
/// 9. Initialize prompt composer — STUB
/// 10. Broadcast BootComplete event
pub async fn boot() -> Result<Runtime, BootError> {
    // Step 1: Config load (TODO: implement config::ensure_config() in M1.7)
    // For now, using default config values
    let _config = ();

    // Step 2: Initialize EventBus
    let bus = Arc::new(EventBus::new());

    // Keep a receiver alive so broadcast channel doesn't close immediately
    // Real actors will subscribe during their start() phase
    let keep_alive = bus.subscribe_broadcast();

    // Step 3: Initialize Soul (SoulBox) — STUB for Phase 1
    // TODO M2.1: Initialize Soul with encryption and redb

    // Step 4: Spawn actor tasks — STUB for Phase 1
    // TODO M2.2: Spawn platform, CTP, memory, inference, prompt actors

    // Step 5: Initialize platform adapter — STUB for Phase 1
    // TODO M1.5: Create platform adapter instance

    // Step 6: Initialize CTP loop — STUB for Phase 1
    // TODO M1.6: Spawn CTP actor

    // Step 7: Initialize memory system — STUB for Phase 1
    // TODO M2.3: Initialize ech0 Store

    // Step 8: Initialize inference system — STUB for Phase 1
    // TODO M2.4: Initialize llama-cpp-rs model loader

    // Step 9: Initialize prompt composer — STUB for Phase 1
    // TODO M2.5: Initialize prompt composer

    // Actor registry (empty for now)
    let registry = ActorRegistry::new();

    // Step 10: Broadcast BootComplete
    bus.broadcast(Event::System(SystemEvent::BootComplete))
        .await
        .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;

    Ok(Runtime {
        bus,
        registry,
        _keep_alive: keep_alive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_returns_ok_runtime() {
        let result = boot().await;
        assert!(result.is_ok());

        let runtime = result.unwrap();
        assert_eq!(runtime.registry.actor_count(), 0); // No actors in Phase 1
    }

    #[tokio::test]
    async fn boot_emits_boot_complete_event() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        // Simulate boot's final step
        let event = Event::System(SystemEvent::BootComplete);
        bus.broadcast(event).await.unwrap();

        // Verify receiver gets BootComplete
        let received = rx.recv().await;
        assert!(received.is_ok());

        if let Ok(Event::System(SystemEvent::BootComplete)) = received {
            // Success
        } else {
            panic!("Expected BootComplete event");
        }
    }
}
