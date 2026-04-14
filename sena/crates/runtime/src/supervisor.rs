//! Supervision loop — monitors actor liveness, handles readiness gate, emits BootComplete.

use crate::boot::BootResult;
use crate::error::RuntimeError;
use bus::{Event, SystemEvent};
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{info, warn};

/// Run the supervision loop.
///
/// Responsibilities:
/// 1. Wait for all expected actors to emit ActorReady (readiness gate)
/// 2. Broadcast BootComplete once readiness gate passes
/// 3. Monitor actor handles for panics or failures
/// 4. Wait for ShutdownSignal on the bus
///
/// This is a stub implementation that:
/// - Logs readiness gate steps
/// - Emits BootComplete immediately (no actual readiness wait)
/// - Returns after a short delay (no shutdown signal handling yet)
pub async fn supervision_loop(boot_result: BootResult) -> Result<(), RuntimeError> {
    info!("SUPERVISOR: supervision loop starting");

    let expected_count = boot_result.expected_actors.len();
    info!(
        expected_actors = ?boot_result.expected_actors,
        "SUPERVISOR: waiting for {} actors to emit ActorReady",
        expected_count
    );

    // Stub: readiness gate implementation
    let readiness_result = await_readiness_gate(&boot_result).await;
    match readiness_result {
        Ok(()) => {
            info!("SUPERVISOR: readiness gate passed");
        }
        Err(e) => {
            warn!("SUPERVISOR: readiness gate failed: {}", e);
            return Err(e);
        }
    }

    // Broadcast BootComplete
    info!("SUPERVISOR: broadcasting BootComplete");
    let _ = boot_result
        .bus
        .broadcast(Event::System(SystemEvent::BootComplete))
        .await;

    info!("SUPERVISOR: BootComplete broadcast successful");

    // Stub: supervision loop (runs for 100ms then exits cleanly)
    info!("SUPERVISOR: entering main loop (stub: exits after 100ms)");
    tokio::time::sleep(Duration::from_millis(100)).await;

    info!("SUPERVISOR: supervision loop exiting (stub)");
    Ok(())
}

/// Wait for all expected actors to emit ActorReady within 30 seconds.
async fn await_readiness_gate(boot_result: &BootResult) -> Result<(), RuntimeError> {
    let expected_actors: HashSet<&'static str> =
        boot_result.expected_actors.iter().copied().collect();

    if expected_actors.is_empty() {
        info!("SUPERVISOR: no actors expected, readiness gate passes immediately");
        return Ok(());
    }

    info!(
        "SUPERVISOR: waiting up to 30s for {} actors",
        expected_actors.len()
    );

    // Stub implementation: log the intent but don't actually wait
    // Real implementation would:
    // 1. Subscribe to broadcast channel
    // 2. Filter for SystemEvent::ActorReady
    // 3. Track which actors have reported ready
    // 4. Return Ok() when all are ready or Err() on timeout

    info!("SUPERVISOR: readiness gate stub — assuming all actors ready");
    Ok(())
}

/// Wait for shutdown signal with optional timeout.
async fn _await_shutdown_signal(
    boot_result: &BootResult,
    timeout_duration: Option<Duration>,
) -> Result<(), RuntimeError> {
    info!("SUPERVISOR: waiting for shutdown signal");

    let shutdown_future = async {
        let mut rx = boot_result.bus.subscribe_broadcast();

        loop {
            match rx.recv().await {
                Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                    info!("SUPERVISOR: shutdown signal received");
                    return Ok::<(), RuntimeError>(());
                }
                Ok(_) => continue,
                Err(e) => {
                    return Err(RuntimeError::SupervisionFailed(format!(
                        "broadcast channel error: {}",
                        e
                    )));
                }
            }
        }
    };

    if let Some(duration) = timeout_duration {
        match timeout(duration, shutdown_future).await {
            Ok(result) => result,
            Err(_) => Err(RuntimeError::SupervisionFailed(
                "shutdown signal timeout".to_string(),
            )),
        }
    } else {
        shutdown_future.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::EventBus;
    use std::sync::Arc;

    #[tokio::test]
    async fn supervision_loop_completes_with_stub() {
        let boot_result = BootResult {
            bus: Arc::new(EventBus::new()),
            encryption: Arc::new(crypto::StubEncryptionLayer),
            actor_handles: vec![],
            expected_actors: vec!["test_actor"],
        };

        let result = supervision_loop(boot_result).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn readiness_gate_with_no_actors() {
        let boot_result = BootResult {
            bus: Arc::new(EventBus::new()),
            encryption: Arc::new(crypto::StubEncryptionLayer),
            actor_handles: vec![],
            expected_actors: vec![],
        };

        let result = await_readiness_gate(&boot_result).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn readiness_gate_with_actors_stub() {
        let boot_result = BootResult {
            bus: Arc::new(EventBus::new()),
            encryption: Arc::new(crypto::StubEncryptionLayer),
            actor_handles: vec![],
            expected_actors: vec!["actor1", "actor2"],
        };

        let result = await_readiness_gate(&boot_result).await;
        // Stub implementation always succeeds
        assert!(result.is_ok());
    }
}
