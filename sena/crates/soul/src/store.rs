//! SoulStore trait: encrypted storage abstraction for Soul subsystem.
//!
//! This trait defines the persistence contract for Soul events, identity signals,
//! and temporal patterns. Implementations will handle encryption, schema management,
//! and concurrent access.

use std::time::SystemTime;

use crate::error::SoulError;
use crate::types::{IdentitySignal, SoulEventRecord, SoulSummary, TemporalPattern};

/// Encrypted storage abstraction for Soul subsystem.
///
/// Implementations must provide:
/// - Encrypted event log append
/// - Identity signal read/write
/// - Temporal pattern persistence
/// - Summary generation from recent events
pub trait SoulStore: Send + Sync {
    /// Write an event to the Soul event log.
    ///
    /// Returns the row ID of the newly written event.
    fn write_event(
        &mut self,
        description: String,
        app_context: Option<String>,
        timestamp: SystemTime,
    ) -> Result<u64, SoulError>;

    /// Read a summary of recent events.
    ///
    /// # Arguments
    /// * `max_events` - Maximum number of events to include in summary
    /// * `max_chars` - Optional character budget for summary content
    fn read_summary(
        &self,
        max_events: usize,
        max_chars: Option<usize>,
    ) -> Result<SoulSummary, SoulError>;

    /// Read a specific event by row ID.
    fn read_event(&self, row_id: u64) -> Result<Option<SoulEventRecord>, SoulError>;

    /// Write an identity signal.
    ///
    /// # Arguments
    /// * `key` - Signal key (e.g., "voice::rate")
    /// * `value` - Signal value
    fn write_identity_signal(&mut self, key: &str, value: &str) -> Result<(), SoulError>;

    /// Read an identity signal by key.
    fn read_identity_signal(&self, key: &str) -> Result<Option<String>, SoulError>;

    /// Read all identity signals.
    fn read_all_identity_signals(&self) -> Result<Vec<IdentitySignal>, SoulError>;

    /// Increment a counter-type identity signal.
    ///
    /// # Arguments
    /// * `key` - Signal key
    /// * `delta` - Increment amount
    fn increment_identity_counter(&mut self, key: &str, delta: u64) -> Result<(), SoulError>;

    /// Write a temporal pattern.
    fn write_temporal_pattern(&mut self, pattern: TemporalPattern) -> Result<(), SoulError>;

    /// Read all temporal patterns.
    fn read_temporal_patterns(&self) -> Result<Vec<TemporalPattern>, SoulError>;

    /// Initialize or open the store with encryption.
    fn initialize(&mut self) -> Result<(), SoulError>;

    /// Close the store cleanly.
    fn close(&mut self) -> Result<(), SoulError>;

    /// Wipe the store from persistent storage and reset to fresh state.
    ///
    /// This closes the database, deletes the underlying file, and re-initializes
    /// a fresh empty store. Used for complete Soul deletion.
    fn wipe(&mut self) -> Result<(), SoulError> {
        self.close()?;
        self.initialize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub implementation for testing trait object construction.
    struct StubStore;

    impl SoulStore for StubStore {
        fn write_event(
            &mut self,
            _description: String,
            _app_context: Option<String>,
            _timestamp: SystemTime,
        ) -> Result<u64, SoulError> {
            Ok(1)
        }

        fn read_summary(
            &self,
            _max_events: usize,
            _max_chars: Option<usize>,
        ) -> Result<SoulSummary, SoulError> {
            Ok(SoulSummary {
                content: String::new(),
                event_count: 0,
            })
        }

        fn read_event(&self, _row_id: u64) -> Result<Option<SoulEventRecord>, SoulError> {
            Ok(None)
        }

        fn write_identity_signal(&mut self, _key: &str, _value: &str) -> Result<(), SoulError> {
            Ok(())
        }

        fn read_identity_signal(&self, _key: &str) -> Result<Option<String>, SoulError> {
            Ok(None)
        }

        fn read_all_identity_signals(&self) -> Result<Vec<IdentitySignal>, SoulError> {
            Ok(Vec::new())
        }

        fn increment_identity_counter(&mut self, _key: &str, _delta: u64) -> Result<(), SoulError> {
            Ok(())
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
    fn soul_store_trait_object_compiles() {
        let store: Box<dyn SoulStore> = Box::new(StubStore);
        let summary = store.read_summary(10, None).expect("read_summary failed");
        assert_eq!(summary.event_count, 0);
    }
}
