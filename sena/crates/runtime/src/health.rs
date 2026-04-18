//! Actor health registry — tracks liveness and status of all spawned actors.

use bus::events::system::{ActorHealth, ActorStatus};
use std::collections::HashMap;
use std::time::Instant;

/// A single actor entry in the registry.
#[derive(Debug)]
pub struct ActorEntry {
    /// Static actor name.
    pub name: &'static str,
    /// Current health status.
    pub status: ActorStatus,
    /// When the actor was registered (proxy for start time).
    pub registered_at: Instant,
}

impl ActorEntry {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            status: ActorStatus::Running,
            registered_at: Instant::now(),
        }
    }

    /// Uptime of this actor entry in seconds.
    pub fn uptime_seconds(&self) -> u64 {
        self.registered_at.elapsed().as_secs()
    }

    /// Convert to the bus-facing `ActorHealth` type.
    pub fn to_health(&self) -> ActorHealth {
        ActorHealth {
            name: self.name.to_string(),
            status: self.status.clone(),
            uptime_seconds: self.uptime_seconds(),
        }
    }
}

/// Registry of all actor health entries for the current boot session.
#[derive(Debug)]
pub struct ActorRegistry {
    entries: HashMap<&'static str, ActorEntry>,
    /// Registry creation time — used to compute overall runtime uptime.
    created_at: Instant,
}

impl ActorRegistry {
    /// Create a new, empty actor registry.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            created_at: Instant::now(),
        }
    }

    /// Register an actor by name. Sets its initial status to `Running`.
    pub fn register(&mut self, name: &'static str) {
        self.entries.insert(name, ActorEntry::new(name));
    }

    /// Mark an actor as failed with the given reason.  
    /// If the actor is not yet registered, it is inserted in a failed state.
    pub fn mark_failed(&mut self, name: &'static str, reason: String) {
        let entry = self
            .entries
            .entry(name)
            .or_insert_with(|| ActorEntry::new(name));
        entry.status = ActorStatus::Failed { reason };
    }

    /// Mark an actor as cleanly stopped.
    pub fn mark_stopped(&mut self, name: &'static str) {
        if let Some(entry) = self.entries.get_mut(name) {
            entry.status = ActorStatus::Stopped;
        }
    }

    /// Return the health snapshot of all registered actors.
    pub fn get_all_health(&self) -> Vec<ActorHealth> {
        self.entries.values().map(|e| e.to_health()).collect()
    }

    /// Overall uptime of the registry in seconds (proxy for runtime uptime).
    pub fn uptime_seconds(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }

    /// Returns `true` if all registered actors are in the `Running` state.
    pub fn all_running(&self) -> bool {
        self.entries
            .values()
            .all(|e| matches!(e.status, ActorStatus::Running))
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

    #[test]
    fn register_and_get_health() {
        let mut registry = ActorRegistry::new();
        registry.register("platform");
        registry.register("soul");

        let health = registry.get_all_health();
        assert_eq!(health.len(), 2);
    }

    #[test]
    fn mark_failed_updates_status() {
        let mut registry = ActorRegistry::new();
        registry.register("inference");
        registry.mark_failed("inference", "model not found".to_string());

        let health = registry.get_all_health();
        let h = health.iter().find(|h| h.name == "inference").unwrap();
        assert!(matches!(h.status, ActorStatus::Failed { .. }));
    }

    #[test]
    fn mark_stopped_updates_status() {
        let mut registry = ActorRegistry::new();
        registry.register("ctp");
        registry.mark_stopped("ctp");

        let health = registry.get_all_health();
        let h = health.iter().find(|h| h.name == "ctp").unwrap();
        assert!(matches!(h.status, ActorStatus::Stopped));
    }

    #[test]
    fn all_running_returns_false_after_failure() {
        let mut registry = ActorRegistry::new();
        registry.register("memory");
        assert!(registry.all_running());
        registry.mark_failed("memory", "disk full".to_string());
        assert!(!registry.all_running());
    }
}
