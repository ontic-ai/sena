//! RedbSoulStore — persistent redb-backed Soul store.

use crate::error::SoulError;
use crate::schema::SchemaV1;
use crate::store::SoulStore;
use crate::types::{IdentitySignal, SoulEventRecord, SoulSummary, TemporalPattern};
use redb::{Database, ReadableTable, TableDefinition};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, info};

const EVENTS_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("soul_events");
const IDENTITY_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("soul_identity");
const TEMPORAL_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("soul_temporal");
const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("soul_meta");

const META_SCHEMA_KEY: &str = "schema";
const META_NEXT_ROW_ID_KEY: &str = "next_row_id";

fn db_error(error: impl std::fmt::Display) -> SoulError {
    SoulError::Database(error.to_string())
}

fn read_u64(bytes: &[u8]) -> Result<u64, SoulError> {
    if bytes.len() != 8 {
        return Err(SoulError::Database(
            "invalid u64 metadata payload".to_string(),
        ));
    }

    let mut raw = [0_u8; 8];
    raw.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(raw))
}

/// Redb-backed encrypted Soul store.
pub struct RedbSoulStore {
    path: PathBuf,
    db: Database,
}

impl RedbSoulStore {
    /// Open or create a redb-backed Soul store.
    pub fn open(path: &Path) -> Result<Self, SoulError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let db = if path.exists() {
            Database::open(path).map_err(db_error)?
        } else {
            Database::create(path).map_err(db_error)?
        };

        let mut store = Self {
            path: path.to_path_buf(),
            db,
        };
        store.initialize()?;
        Ok(store)
    }

    fn ensure_tables(&self) -> Result<(), SoulError> {
        let write_txn = self.db.begin_write().map_err(db_error)?;

        {
            write_txn.open_table(EVENTS_TABLE).map_err(db_error)?;
            write_txn.open_table(IDENTITY_TABLE).map_err(db_error)?;
            write_txn.open_table(TEMPORAL_TABLE).map_err(db_error)?;
            let mut meta_table = write_txn.open_table(META_TABLE).map_err(db_error)?;

            if meta_table.get(META_SCHEMA_KEY).map_err(db_error)?.is_none() {
                let schema = serde_json::to_vec(&SchemaV1::default())
                    .map_err(|error| SoulError::Database(error.to_string()))?;
                meta_table
                    .insert(META_SCHEMA_KEY, schema.as_slice())
                    .map_err(db_error)?;
            }

            if meta_table
                .get(META_NEXT_ROW_ID_KEY)
                .map_err(db_error)?
                .is_none()
            {
                let next_row_id = 1_u64.to_le_bytes();
                meta_table
                    .insert(META_NEXT_ROW_ID_KEY, next_row_id.as_slice())
                    .map_err(db_error)?;
            }
        }

        write_txn.commit().map_err(db_error)?;
        Ok(())
    }

    fn read_meta_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let meta_table = read_txn.open_table(META_TABLE).map_err(db_error)?;
        Ok(meta_table
            .get(key)
            .map_err(db_error)?
            .map(|value| value.value().to_vec()))
    }

    fn collect_event_keys(&self) -> Result<Vec<u64>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(EVENTS_TABLE).map_err(db_error)?;
        let mut keys = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (key, _) = entry.map_err(db_error)?;
            keys.push(key.value());
        }

        Ok(keys)
    }

    fn collect_identity_keys(&self) -> Result<Vec<String>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(IDENTITY_TABLE).map_err(db_error)?;
        let mut keys = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (key, _) = entry.map_err(db_error)?;
            keys.push(key.value().to_string());
        }

        Ok(keys)
    }

    fn collect_temporal_keys(&self) -> Result<Vec<String>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(TEMPORAL_TABLE).map_err(db_error)?;
        let mut keys = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (key, _) = entry.map_err(db_error)?;
            keys.push(key.value().to_string());
        }

        Ok(keys)
    }

    fn collect_meta_keys(&self) -> Result<Vec<String>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(META_TABLE).map_err(db_error)?;
        let mut keys = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (key, _) = entry.map_err(db_error)?;
            keys.push(key.value().to_string());
        }

        Ok(keys)
    }
}

impl SoulStore for RedbSoulStore {
    fn write_event(
        &mut self,
        description: String,
        app_context: Option<String>,
        timestamp: SystemTime,
    ) -> Result<u64, SoulError> {
        let write_txn = self.db.begin_write().map_err(db_error)?;
        let row_id = {
            let meta_table = write_txn.open_table(META_TABLE).map_err(db_error)?;
            let next_row_id_bytes = meta_table
                .get(META_NEXT_ROW_ID_KEY)
                .map_err(db_error)?
                .ok_or_else(|| SoulError::Database("missing next row id metadata".to_string()))?;
            read_u64(next_row_id_bytes.value())?
        };

        let record = SoulEventRecord {
            row_id,
            description,
            app_context,
            timestamp,
        };
        let payload =
            serde_json::to_vec(&record).map_err(|error| SoulError::Database(error.to_string()))?;

        {
            let mut events_table = write_txn.open_table(EVENTS_TABLE).map_err(db_error)?;
            events_table
                .insert(row_id, payload.as_slice())
                .map_err(db_error)?;
        }

        {
            let mut meta_table = write_txn.open_table(META_TABLE).map_err(db_error)?;
            let next_row_id = row_id.saturating_add(1).to_le_bytes();
            meta_table
                .insert(META_NEXT_ROW_ID_KEY, next_row_id.as_slice())
                .map_err(db_error)?;
        }

        write_txn.commit().map_err(db_error)?;

        debug!(row_id, "soul event written");
        Ok(row_id)
    }

    fn read_summary(
        &self,
        max_events: usize,
        max_chars: Option<usize>,
    ) -> Result<SoulSummary, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(EVENTS_TABLE).map_err(db_error)?;
        let mut events = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (_, value) = entry.map_err(db_error)?;
            let event: SoulEventRecord = serde_json::from_slice(value.value())
                .map_err(|error| SoulError::Database(error.to_string()))?;
            events.push(event);
        }

        let mut content = String::new();

        for event in events.iter().rev().take(max_events).rev() {
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
            event_count: events.len().min(max_events),
        })
    }

    fn read_event(&self, row_id: u64) -> Result<Option<SoulEventRecord>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(EVENTS_TABLE).map_err(db_error)?;
        let value = match table.get(row_id).map_err(db_error)? {
            Some(value) => value,
            None => return Ok(None),
        };

        let event = serde_json::from_slice(value.value())
            .map_err(|error| SoulError::Database(error.to_string()))?;
        Ok(Some(event))
    }

    fn write_identity_signal(&mut self, key: &str, value: &str) -> Result<(), SoulError> {
        let write_txn = self.db.begin_write().map_err(db_error)?;
        {
            let mut table = write_txn.open_table(IDENTITY_TABLE).map_err(db_error)?;
            table.insert(key, value.as_bytes()).map_err(db_error)?;
        }
        write_txn.commit().map_err(db_error)?;
        debug!(key, "identity signal written");
        Ok(())
    }

    fn read_identity_signal(&self, key: &str) -> Result<Option<String>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(IDENTITY_TABLE).map_err(db_error)?;
        let Some(value) = table.get(key).map_err(db_error)? else {
            return Ok(None);
        };

        let stored = std::str::from_utf8(value.value())
            .map_err(|error| SoulError::Database(error.to_string()))?;
        Ok(Some(stored.to_string()))
    }

    fn read_all_identity_signals(&self) -> Result<Vec<IdentitySignal>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(IDENTITY_TABLE).map_err(db_error)?;
        let mut signals = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (key, value) = entry.map_err(db_error)?;
            let stored = std::str::from_utf8(value.value())
                .map_err(|error| SoulError::Database(error.to_string()))?;
            signals.push(IdentitySignal {
                key: key.value().to_string(),
                value: stored.to_string(),
            });
        }

        Ok(signals)
    }

    fn increment_identity_counter(&mut self, key: &str, delta: u64) -> Result<(), SoulError> {
        let current: u64 = self
            .read_identity_signal(key)?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let new_val = current.saturating_add(delta);
        self.write_identity_signal(key, &new_val.to_string())
    }

    fn write_temporal_pattern(&mut self, pattern: TemporalPattern) -> Result<(), SoulError> {
        let payload =
            serde_json::to_vec(&pattern).map_err(|error| SoulError::Database(error.to_string()))?;
        let write_txn = self.db.begin_write().map_err(db_error)?;
        {
            let mut table = write_txn.open_table(TEMPORAL_TABLE).map_err(db_error)?;
            table
                .insert(pattern.pattern_type.as_str(), payload.as_slice())
                .map_err(db_error)?;
        }
        write_txn.commit().map_err(db_error)?;
        Ok(())
    }

    fn read_temporal_patterns(&self) -> Result<Vec<TemporalPattern>, SoulError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(TEMPORAL_TABLE).map_err(db_error)?;
        let mut patterns = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (_, value) = entry.map_err(db_error)?;
            let pattern = serde_json::from_slice(value.value())
                .map_err(|error| SoulError::Database(error.to_string()))?;
            patterns.push(pattern);
        }

        Ok(patterns)
    }

    fn load_schema(&self) -> Result<Option<SchemaV1>, SoulError> {
        let payload = match self.read_meta_bytes(META_SCHEMA_KEY)? {
            Some(payload) => payload,
            None => return Ok(None),
        };

        let schema = serde_json::from_slice(&payload)
            .map_err(|error| SoulError::Database(error.to_string()))?;
        Ok(Some(schema))
    }

    fn save_schema(&mut self, schema: &SchemaV1) -> Result<(), SoulError> {
        let payload =
            serde_json::to_vec(schema).map_err(|error| SoulError::Database(error.to_string()))?;
        let write_txn = self.db.begin_write().map_err(db_error)?;
        {
            let mut meta_table = write_txn.open_table(META_TABLE).map_err(db_error)?;
            meta_table
                .insert(META_SCHEMA_KEY, payload.as_slice())
                .map_err(db_error)?;
        }
        write_txn.commit().map_err(db_error)?;
        Ok(())
    }

    fn initialize(&mut self) -> Result<(), SoulError> {
        self.ensure_tables()?;
        info!(path = %self.path.display(), "RedbSoulStore initialized");
        Ok(())
    }

    fn close(&mut self) -> Result<(), SoulError> {
        info!(path = %self.path.display(), "RedbSoulStore closed");
        Ok(())
    }

    fn wipe(&mut self) -> Result<(), SoulError> {
        let event_keys = self.collect_event_keys()?;
        let identity_keys = self.collect_identity_keys()?;
        let temporal_keys = self.collect_temporal_keys()?;
        let meta_keys = self.collect_meta_keys()?;

        let write_txn = self.db.begin_write().map_err(db_error)?;
        {
            let mut event_table = write_txn.open_table(EVENTS_TABLE).map_err(db_error)?;
            for key in event_keys {
                event_table.remove(key).map_err(db_error)?;
            }
        }
        {
            let mut identity_table = write_txn.open_table(IDENTITY_TABLE).map_err(db_error)?;
            for key in identity_keys {
                identity_table.remove(key.as_str()).map_err(db_error)?;
            }
        }
        {
            let mut temporal_table = write_txn.open_table(TEMPORAL_TABLE).map_err(db_error)?;
            for key in temporal_keys {
                temporal_table.remove(key.as_str()).map_err(db_error)?;
            }
        }
        {
            let mut meta_table = write_txn.open_table(META_TABLE).map_err(db_error)?;
            for key in meta_keys {
                meta_table.remove(key.as_str()).map_err(db_error)?;
            }
        }
        write_txn.commit().map_err(db_error)?;

        self.initialize()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn redb_store_write_and_read_event() {
        let dir = tempdir().expect("failed to create tempdir");
        let db_path = dir.path().join("soul.redb");
        let mut store = RedbSoulStore::open(&db_path).expect("open should succeed");

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
        let dir = tempdir().expect("failed to create tempdir");
        let db_path = dir.path().join("soul.redb");
        let mut store = RedbSoulStore::open(&db_path).expect("open should succeed");
        store
            .write_identity_signal("key", "value")
            .expect("write should succeed");
        let val = store
            .read_identity_signal("key")
            .expect("read should succeed");
        assert_eq!(val, Some("value".to_string()));
    }

    #[test]
    fn redb_store_persists_schema_across_reopen() {
        let dir = tempdir().expect("failed to create tempdir");
        let db_path = dir.path().join("soul.redb");

        {
            let mut store = RedbSoulStore::open(&db_path).expect("open should succeed");
            let mut schema = store
                .load_schema()
                .expect("load should succeed")
                .expect("schema should exist");
            schema.session_count = 4;
            schema.name = "Sena".to_string();
            store.save_schema(&schema).expect("save should succeed");
        }

        let reopened = RedbSoulStore::open(&db_path).expect("reopen should succeed");
        let schema = reopened
            .load_schema()
            .expect("load should succeed")
            .expect("schema should exist");
        assert_eq!(schema.session_count, 4);
        assert_eq!(schema.name, "Sena");
    }
}
