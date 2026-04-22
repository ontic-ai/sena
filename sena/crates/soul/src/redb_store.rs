//! RedbSoulStore — redb-backed encrypted Soul store.
//!
//! BONES stub: this module provides the `RedbSoulStore` type that implements
//! `SoulStore`. The current implementation is an in-memory stub for initial
//! integration. The real redb + encryption implementation is deferred to
//! the encrypted storage phase.

use crate::error::SoulError;
use crate::store::SoulStore;
use crate::types::{IdentitySignal, SoulEventRecord, SoulSummary, TemporalPattern};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, info};

/// Redb-backed encrypted Soul store.
///
/// In BONES: operates as an in-memory store.
/// Production: encrypts all writes using the crypto layer and persists to `redb`.
pub struct RedbSoulStore {
    /// Path to the store file (used in production; ignored in BONES stub).
    path: PathBuf,
    /// In-memory event log.
    events: Vec<SoulEventRecord>,
    /// Next row ID counter.
    next_row_id: u64,
    /// Identity signal map (key → value).
    identity_signals: HashMap<String, String>,
    /// Temporal patterns.
    temporal_patterns: Vec<TemporalPattern>,
}

impl RedbSoulStore {
    /// Create a new store pointing at the given database path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            events: Vec::new(),
            next_row_id: 1,
            identity_signals: HashMap::new(),
            temporal_patterns: Vec::new(),
        }
    }
}

impl SoulStore for RedbSoulStore {
    fn write_event(
        &mut self,
        description: String,
        app_context: Option<String>,
        timestamp: SystemTime,
    ) -> Result<u64, SoulError> {
        let row_id = self.next_row_id;
        self.next_row_id += 1;

        self.events.push(SoulEventRecord {
            row_id,
            description,
            app_context,
            timestamp,
        });

        debug!(row_id, "soul event written (BONES stub)");
        Ok(row_id)
    }

    fn read_summary(
        &self,
        max_events: usize,
        max_chars: Option<usize>,
    ) -> Result<SoulSummary, SoulError> {
        let recent: Vec<_> = self.events.iter().rev().take(max_events).collect();
        let mut content = String::new();

        for event in recent.iter().rev() {
            let line = format!("- {}\n", event.description);
            if let Some(limit) = max_chars
                && content.len() + line.len() > limit
            {
                break;
            }
            content.push_str(&line);
        }

        Ok(SoulSummary {
            content,
            event_count: self.events.len().min(max_events),
        })
    }

    fn read_event(&self, row_id: u64) -> Result<Option<SoulEventRecord>, SoulError> {
        Ok(self.events.iter().find(|e| e.row_id == row_id).cloned())
    }

    fn write_identity_signal(&mut self, key: &str, value: &str) -> Result<(), SoulError> {
        debug!(key, "identity signal written (BONES stub)");
        self.identity_signals
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn read_identity_signal(&self, key: &str) -> Result<Option<String>, SoulError> {
        Ok(self.identity_signals.get(key).cloned())
    }

    fn read_all_identity_signals(&self) -> Result<Vec<IdentitySignal>, SoulError> {
        Ok(self
            .identity_signals
            .iter()
            .map(|(k, v)| IdentitySignal {
                key: k.clone(),
                value: v.clone(),
            })
            .collect())
    }

    fn increment_identity_counter(&mut self, key: &str, delta: u64) -> Result<(), SoulError> {
        let current: u64 = self
            .identity_signals
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let new_val = current.saturating_add(delta);
        self.identity_signals
            .insert(key.to_string(), new_val.to_string());
        Ok(())
    }

    fn write_temporal_pattern(&mut self, pattern: TemporalPattern) -> Result<(), SoulError> {
        // Update existing or append
        if let Some(existing) = self
            .temporal_patterns
            .iter_mut()
            .find(|p| p.pattern_type == pattern.pattern_type)
        {
            *existing = pattern;
        } else {
            self.temporal_patterns.push(pattern);
        }
        Ok(())
    }

    fn read_temporal_patterns(&self) -> Result<Vec<TemporalPattern>, SoulError> {
        Ok(self.temporal_patterns.clone())
    }

    fn initialize(&mut self) -> Result<(), SoulError> {
        info!(path = ?self.path, "RedbSoulStore initialized (BONES stub)");
        Ok(())
    }

    fn close(&mut self) -> Result<(), SoulError> {
        info!("RedbSoulStore closed (BONES stub)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redb_store_write_and_read_event() {
        let mut store = RedbSoulStore::new("/tmp/test-soul.db");
        store.initialize().expect("initialize should succeed");

        let row_id = store
            .write_event("test event".to_string(), None, SystemTime::UNIX_EPOCH)
            .expect("write should succeed");

        assert_eq!(row_id, 1);
        let event = store.read_event(row_id).expect("read should succeed");
        assert!(event.is_some());
        assert_eq!(event.unwrap().description, "test event");
    }

    #[test]
    fn redb_store_identity_signals() {
        let mut store = RedbSoulStore::new("/tmp/test-soul.db");
        store
            .write_identity_signal("key", "value")
            .expect("write should succeed");
        let val = store
            .read_identity_signal("key")
            .expect("read should succeed");
        assert_eq!(val, Some("value".to_string()));
    }
}
