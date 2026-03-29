//! Actor registry for lifecycle management.

use std::collections::HashMap;
use std::time::Duration;
use tokio::task::JoinHandle;

/// Registry of spawned actor tasks for liveness monitoring.
///
/// Tracks JoinHandles for all spawned actors. Provides wait-all functionality
/// for graceful shutdown. Liveness monitoring (restart policy) is a stub for Phase 1.
pub struct ActorRegistry {
    /// Actor name -> task handle.
    actors: HashMap<&'static str, JoinHandle<()>>,
}

impl ActorRegistry {
    /// Create a new empty actor registry.
    pub fn new() -> Self {
        Self {
            actors: HashMap::new(),
        }
    }

    /// Register a spawned actor task.
    pub fn register(&mut self, name: &'static str, handle: JoinHandle<()>) {
        self.actors.insert(name, handle);
    }

    /// Wait for all actors to complete with a timeout.
    ///
    /// Returns a vec of (actor_name, join_result) for each actor.
    /// Actors that don't complete within timeout will have their handles aborted.
    ///
    /// Note: Timeout handling is best-effort in Phase 1. In production, actors
    /// that don't respond to shutdown signals within timeout are considered failed.
    pub async fn wait_all(
        &mut self,
        timeout: Duration,
    ) -> Vec<(&'static str, Result<(), String>)> {
        let mut results = Vec::new();

        for (name, handle) in self.actors.drain() {
            let result = tokio::time::timeout(timeout, handle).await;

            match result {
                Ok(Ok(())) => {
                    // Actor completed successfully
                    results.push((name, Ok(())));
                }
                Ok(Err(join_error)) => {
                    // Actor panicked or was cancelled
                    results.push((name, Err(format!("join error: {}", join_error))));
                }
                Err(_) => {
                    // Timeout elapsed - actor didn't stop in time
                    results.push((name, Err(format!("timeout after {}s", timeout.as_secs()))));
                }
            }
        }

        results
    }

    /// Get count of registered actors.
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }
}

impl Default for ActorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn registry_starts_empty() {
        let registry = ActorRegistry::new();
        assert_eq!(registry.actor_count(), 0);
    }

    #[tokio::test]
    async fn register_increments_count() {
        let mut registry = ActorRegistry::new();

        let handle1 = tokio::spawn(async {});
        registry.register("actor1", handle1);
        assert_eq!(registry.actor_count(), 1);

        let handle2 = tokio::spawn(async {});
        registry.register("actor2", handle2);
        assert_eq!(registry.actor_count(), 2);
    }

    #[tokio::test]
    async fn wait_all_completes_for_finished_actors() {
        let mut registry = ActorRegistry::new();

        let handle = tokio::spawn(async {
            // Immediate completion
        });
        registry.register("fast_actor", handle);

        let results = registry.wait_all(Duration::from_secs(1)).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "fast_actor");
        assert!(results[0].1.is_ok());
    }

    #[tokio::test]
    async fn wait_all_drains_registry() {
        let mut registry = ActorRegistry::new();

        let handle1 = tokio::spawn(async {});
        let handle2 = tokio::spawn(async {});
        registry.register("actor1", handle1);
        registry.register("actor2", handle2);

        assert_eq!(registry.actor_count(), 2);

        let _results = registry.wait_all(Duration::from_secs(1)).await;

        // Registry should be empty after wait_all (drain)
        assert_eq!(registry.actor_count(), 0);
    }
}
