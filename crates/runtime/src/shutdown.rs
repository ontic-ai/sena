//! Graceful shutdown protocol.

use std::time::Duration;

use bus::{Event, SystemEvent};

use crate::boot::Runtime;

/// Shutdown errors.
#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    /// Broadcast of shutdown signal failed.
    #[error("shutdown signal broadcast failed: {0}")]
    BroadcastFailed(String),

    /// One or more actors failed to stop within timeout.
    #[error("actor shutdown timeout: {0}")]
    ActorTimeout(String),
}

/// Graceful shutdown per architecture §4.2.
///
/// Steps:
/// 1. Broadcast ShutdownSignal event
/// 2. Wait for all actors to complete (with timeout)
/// 3. Log any actors that failed to stop
/// 4. Return success or timeout error
///
/// Default timeout: 5 seconds per §4.2.
pub async fn shutdown(mut runtime: Runtime, timeout: Duration) -> Result<(), ShutdownError> {
    // Step 1: Broadcast ShutdownSignal
    runtime
        .bus
        .broadcast(Event::System(SystemEvent::ShutdownSignal))
        .await
        .map_err(|e| ShutdownError::BroadcastFailed(e.to_string()))?;

    // Step 2: Wait for all actors
    let results = runtime.registry.wait_all(timeout).await;

    // Step 3: Log actors that failed (or timed out)
    let mut failed_actors = Vec::new();
    for (name, result) in results {
        if let Err(e) = result {
            eprintln!("Actor '{}' failed to stop cleanly: {}", name, e);
            failed_actors.push(name);
        }
    }

    // Step 4: Return result
    if failed_actors.is_empty() {
        Ok(())
    } else {
        Err(ShutdownError::ActorTimeout(format!(
            "Actors did not stop within {}s: {:?}",
            timeout.as_secs(),
            failed_actors
        )))
    }
}

/// Wait for OS SIGINT (Ctrl+C) signal.
///
/// Cross-platform using tokio::signal::ctrl_c().
pub async fn wait_for_sigint() -> Result<(), std::io::Error> {
    tokio::signal::ctrl_c().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SenaConfig;
    use crate::registry::ActorRegistry;
    use bus::EventBus;
    use crypto::MasterKey;
    use std::sync::Arc;

    #[tokio::test]
    async fn shutdown_broadcasts_shutdown_signal() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();
        let keep_alive = bus.subscribe_broadcast();

        // Simulate runtime
        let registry = ActorRegistry::new();
        let tray_manager =
            crate::tray::TrayManager::new(Arc::clone(&bus), tokio::runtime::Handle::current());
        let runtime = Runtime {
            bus: bus.clone(),
            registry,
            config: SenaConfig::default(),
            tray_manager,
            is_first_boot: false,
            master_key: MasterKey::from_bytes([0u8; 32]),
            _keep_alive: keep_alive,
        };

        // Spawn shutdown task
        let shutdown_handle =
            tokio::spawn(async move { shutdown(runtime, Duration::from_secs(1)).await });

        // Verify ShutdownSignal is broadcast
        let event = rx.recv().await;
        assert!(event.is_ok());

        if let Ok(Event::System(SystemEvent::ShutdownSignal)) = event {
            // Success
        } else {
            panic!("Expected ShutdownSignal event");
        }

        // Let shutdown complete
        let _ = shutdown_handle.await;
    }

    #[tokio::test]
    async fn shutdown_completes_with_no_actors() {
        // Construct Runtime manually to avoid writing to real config dir
        let bus = Arc::new(EventBus::new());
        let keep_alive = bus.subscribe_broadcast();
        let registry = ActorRegistry::new();
        let tray_manager =
            crate::tray::TrayManager::new(Arc::clone(&bus), tokio::runtime::Handle::current());
        let runtime = Runtime {
            bus,
            registry,
            config: SenaConfig::default(),
            tray_manager,
            is_first_boot: false,
            master_key: MasterKey::from_bytes([0u8; 32]),
            _keep_alive: keep_alive,
        };

        let result = shutdown(runtime, Duration::from_secs(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn shutdown_waits_for_actor_completion() {
        let bus = Arc::new(EventBus::new());
        let mut registry = ActorRegistry::new();

        // Keep a receiver alive to prevent broadcast channel from closing
        let _rx = bus.subscribe_broadcast();

        // Spawn a fast-completing actor
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        });
        registry.register("fast_actor", handle);

        let tray_manager =
            crate::tray::TrayManager::new(Arc::clone(&bus), tokio::runtime::Handle::current());

        let runtime = Runtime {
            bus: bus.clone(),
            registry,
            config: SenaConfig::default(),
            tray_manager,
            is_first_boot: false,
            master_key: MasterKey::from_bytes([0u8; 32]),
            _keep_alive: _rx,
        };

        let result = shutdown(runtime, Duration::from_secs(1)).await;
        assert!(result.is_ok());
    }
}
