//! Runtime health and supervisor management.

pub use bus::events::system::{ActorHealth as ActorEntry, ActorStatus};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Internal health state tracked by supervisor.
#[derive(Debug, Clone)]
struct HealthEntry {
    /// Last recorded status.
    status: ActorStatus,
    /// Last activity timestamp.
    last_seen: Instant,
}

/// Registry of actor health information.
#[derive(Debug, Clone, Default)]
pub struct ActorRegistry {
    actors: HashMap<String, HealthEntry>,
    start_time: Option<Instant>,
}

impl ActorRegistry {
    pub fn new() -> Self {
        Self {
            actors: HashMap::new(),
            start_time: Some(Instant::now()),
        }
    }

    pub fn register(&mut self, name: &str) {
        self.actors.insert(
            name.to_string(),
            HealthEntry {
                status: ActorStatus::Starting,
                last_seen: Instant::now(),
            },
        );
    }

    pub fn mark_running(&mut self, name: &str) {
        if let Some(entry) = self.actors.get_mut(name) {
            entry.status = ActorStatus::Ready;
            entry.last_seen = Instant::now();
        }
    }

    pub fn mark_failed(&mut self, name: &str, reason: String) {
        if let Some(entry) = self.actors.get_mut(name) {
            entry.status = ActorStatus::Failed { reason };
            entry.last_seen = Instant::now();
        }
    }

    pub fn mark_stopped(&mut self, name: &str) {
        if let Some(entry) = self.actors.get_mut(name) {
            entry.status = ActorStatus::Idle;
            entry.last_seen = Instant::now();
        }
    }

    pub fn get_all_health(&self) -> Vec<ActorEntry> {
        self.actors
            .iter()
            .map(|(name, entry)| ActorEntry {
                name: name.clone(),
                status: entry.status.clone(),
                last_seen: entry.last_seen,
            })
            .collect()
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.start_time.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }
}

/// Tracks the health of all registered actors.
#[derive(Debug, Clone, Default)]
pub struct Supervisor {
    actors: Arc<Mutex<ActorRegistry>>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            actors: Arc::new(Mutex::new(ActorRegistry::new())),
        }
    }

    pub fn update_status(&self, actor: &str, status: ActorStatus) {
        let mut registry = match self.actors.lock() {
            Ok(registry) => registry,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(entry) = registry.actors.get_mut(actor) {
            entry.status = status;
            entry.last_seen = Instant::now();
        } else {
            registry.actors.insert(
                actor.to_string(),
                HealthEntry {
                    status,
                    last_seen: Instant::now(),
                },
            );
        }
    }

    pub fn report(&self) -> (Vec<ActorEntry>, u64) {
        let registry = match self.actors.lock() {
            Ok(registry) => registry,
            Err(poisoned) => poisoned.into_inner(),
        };
        (registry.get_all_health(), registry.uptime_seconds())
    }

    pub fn mark_ready(&self, actor: &str) {
        self.update_status(actor, ActorStatus::Ready);
    }
}
