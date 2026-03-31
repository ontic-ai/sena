//! Redb table definitions and schema migration for the Soul store.
//!
//! Schema version: 2
//!
//! Tables:
//! - `_schema_meta`      : (&str -> u64)  â€” schema version metadata
//! - `event_log`         : (u64 -> &[u8]) â€” sequential event log (UTF-8 descriptions)
//! - `identity_signals`  : (&str -> &str) â€” behavioral identity signals
//! - `preferences`       : (&str -> &str) â€” user preference key-value store

use redb::TableDefinition;

use crate::error::SoulError;

/// Current schema version. Increment when adding new tables or changing layout.
pub const SCHEMA_VERSION: u64 = 2;

const VERSION_KEY: &str = "schema_version";

/// Schema version metadata table.
pub const SCHEMA_META: TableDefinition<&str, u64> = TableDefinition::new("_schema_meta");

/// Append-only event log.
/// Key: auto-incrementing u64 sequence number.
/// Value: UTF-8 encoded event description bytes.
pub const EVENT_LOG: TableDefinition<u64, &[u8]> = TableDefinition::new("event_log");

/// Behavioral identity signals derived from platform observations.
/// Key: signal name (e.g. "frequent_app").
/// Value: signal value (e.g. "Code").
pub const IDENTITY_SIGNALS: TableDefinition<&str, &str> = TableDefinition::new("identity_signals");

/// User preference key-value store (TOML-encoded values).
/// Key: dot-separated preference path (e.g. "ctp.trigger_interval_secs").
/// Value: TOML-serialized value.
pub const PREFERENCES: TableDefinition<&str, &str> = TableDefinition::new("preferences");

/// User identity table — stores user-provided name from first-boot onboarding.
/// Single-row table. Key: "user_name". Value: UTF-8 encoded name.
pub const USER_IDENTITY: TableDefinition<&str, &str> = TableDefinition::new("user_identity");

/// Apply all schema migrations up to [`SCHEMA_VERSION`].
///
/// Safe to call on an existing database â€” only applies missing migrations.
pub fn apply_schema(db: &redb::Database) -> Result<(), SoulError> {
    let current_version: u64 = {
        match db.begin_read() {
            Ok(read_txn) => match read_txn.open_table(SCHEMA_META) {
                Ok(meta) => meta.get(VERSION_KEY)?.map(|g| g.value()).unwrap_or(0),
                Err(_) => 0,
            },
            Err(_) => 0,
        }
    };

    if current_version >= SCHEMA_VERSION {
        return Ok(());
    }

    if current_version < 1 {
        let write_txn = db.begin_write()?;
        {
            let _t = write_txn.open_table(EVENT_LOG)?;
        }
        {
            let _t = write_txn.open_table(IDENTITY_SIGNALS)?;
        }
        {
            let _t = write_txn.open_table(PREFERENCES)?;
        }
        {
            let mut meta = write_txn.open_table(SCHEMA_META)?;
            meta.insert(VERSION_KEY, 1u64)?;
        }
        write_txn.commit()?;
    }

    if current_version < 2 {
        let write_txn = db.begin_write()?;
        {
            let _t = write_txn.open_table(USER_IDENTITY)?;
        }
        {
            let mut meta = write_txn.open_table(SCHEMA_META)?;
            meta.insert(VERSION_KEY, 2u64)?;
        }
        write_txn.commit()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use redb::ReadableTable;
    use tempfile::tempdir;

    fn open_test_db(dir: &std::path::Path) -> redb::Database {
        redb::Database::create(dir.join("test.redb")).expect("should create db")
    }

    #[test]
    fn apply_schema_creates_all_tables_on_fresh_db() {
        let dir = tempdir().expect("should create temp dir");
        let db = open_test_db(dir.path());

        apply_schema(&db).expect("apply_schema should succeed on fresh db");

        let read_txn = db.begin_read().expect("should begin read");
        let meta = read_txn
            .open_table(SCHEMA_META)
            .expect("meta table should exist");
        let version = meta.get(VERSION_KEY).expect("get should succeed");
        assert_eq!(version.unwrap().value(), SCHEMA_VERSION);

        read_txn
            .open_table(EVENT_LOG)
            .expect("event_log should exist");
        read_txn
            .open_table(IDENTITY_SIGNALS)
            .expect("identity_signals should exist");
        read_txn
            .open_table(PREFERENCES)
            .expect("preferences should exist");
    }

    #[test]
    fn apply_schema_is_idempotent() {
        let dir = tempdir().expect("should create temp dir");
        let db = open_test_db(dir.path());

        apply_schema(&db).expect("first apply should succeed");
        apply_schema(&db).expect("second apply should succeed (idempotent)");

        let read_txn = db.begin_read().expect("should begin read");
        let meta = read_txn.open_table(SCHEMA_META).expect("meta should exist");
        let version = meta.get(VERSION_KEY).expect("get should succeed");
        assert_eq!(version.unwrap().value(), SCHEMA_VERSION);
    }

    #[test]
    fn event_log_table_supports_insert_retrieve_and_reverse_iter() {
        let dir = tempdir().expect("should create temp dir");
        let db = open_test_db(dir.path());
        apply_schema(&db).expect("schema should apply");

        let write_txn = db.begin_write().expect("should begin write");
        {
            let mut log = write_txn
                .open_table(EVENT_LOG)
                .expect("should open event_log");
            log.insert(0u64, b"first event".as_slice())
                .expect("should insert");
            log.insert(1u64, b"second event".as_slice())
                .expect("should insert");
        }
        write_txn.commit().expect("should commit");

        let read_txn = db.begin_read().expect("should begin read");
        let log = read_txn
            .open_table(EVENT_LOG)
            .expect("should open event_log");
        let val = log.get(0u64).expect("should get").expect("should exist");
        assert_eq!(val.value(), b"first event");

        let entries: Vec<String> = log
            .iter()
            .expect("iter should succeed")
            .rev()
            .take(1)
            .map(
                |r: Result<(redb::AccessGuard<'_, u64>, redb::AccessGuard<'_, &[u8]>), _>| {
                    let (_, v) = r.expect("item should be ok");
                    String::from_utf8_lossy(v.value()).into_owned()
                },
            )
            .collect();
        assert_eq!(entries[0], "second event");
    }

    #[test]
    fn identity_signals_supports_insert_and_retrieve() {
        let dir = tempdir().expect("should create temp dir");
        let db = open_test_db(dir.path());
        apply_schema(&db).expect("schema should apply");

        let write_txn = db.begin_write().expect("should begin write");
        {
            let mut signals = write_txn.open_table(IDENTITY_SIGNALS).expect("should open");
            signals
                .insert("frequent_app", "Code")
                .expect("should insert");
        }
        write_txn.commit().expect("should commit");

        let read_txn = db.begin_read().expect("should begin read");
        let signals = read_txn.open_table(IDENTITY_SIGNALS).expect("should open");
        let val = signals
            .get("frequent_app")
            .expect("should get")
            .expect("should exist");
        assert_eq!(val.value(), "Code");
    }
}
