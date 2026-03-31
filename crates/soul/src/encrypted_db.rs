use crate::error::SoulError;
use crypto::MasterKey;
use std::path::{Path, PathBuf};

/// A redb database with transparent at-rest encryption.
///
/// On open, the persistent encrypted file is decrypted to a temporary working
/// copy. All redb operations happen against this working copy. On flush or
/// close, the working copy is encrypted back to the persistent location.
///
/// The working file (`.working` extension) is cleaned up on close or drop.
pub struct EncryptedDb {
    /// Path to the persistent encrypted file on disk.
    encrypted_path: PathBuf,
    /// Path to the temporary decrypted working copy.
    working_path: PathBuf,
    /// The open redb database operating on the working path.
    db: Option<redb::Database>,
    /// Master key for envelope encryption operations.
    master_key: MasterKey,
}

impl EncryptedDb {
    /// Open or create an encrypted redb database.
    ///
    /// If `encrypted_path` exists, it is decrypted to a working copy and opened.
    /// If it does not exist, a new empty database is created.
    ///
    /// The `master_key` is copied (via `from_bytes`) so the caller retains its own copy.
    pub fn open(encrypted_path: &Path, master_key: &MasterKey) -> Result<Self, SoulError> {
        let working_path = encrypted_path.with_extension("working");

        // If a stale working file exists from a previous crash, remove it.
        // We always trust the encrypted file as the source of truth.
        if working_path.exists() {
            std::fs::remove_file(&working_path)?;
        }

        let db = if encrypted_path.exists() {
            // Decrypt existing database to working path
            let plaintext = crypto::file::read_encrypted_file(encrypted_path, master_key)?;
            std::fs::write(&working_path, &plaintext)?;
            redb::Database::open(&working_path)?
        } else {
            // Create new database at working path
            redb::Database::create(&working_path)?
        };

        let owned_key = MasterKey::from_bytes(*master_key.as_bytes());

        Ok(Self {
            encrypted_path: encrypted_path.to_path_buf(),
            working_path,
            db: Some(db),
            master_key: owned_key,
        })
    }

    /// Get a reference to the underlying redb database for read/write operations.
    ///
    /// Returns an error if the database has been closed.
    /// Used by Soul schema and event log operations (M2.5).
    #[allow(dead_code)]
    pub(crate) fn db(&self) -> Result<&redb::Database, SoulError> {
        self.db
            .as_ref()
            .ok_or_else(|| SoulError::Database("database is closed".to_string()))
    }

    /// Encrypt the current database state and write to persistent storage.
    ///
    /// This temporarily closes and reopens the database to release the file
    /// handle for reading. During the brief window between close and reopen,
    /// the database is unavailable.
    pub fn flush(&mut self) -> Result<(), SoulError> {
        // Close the database to release the file handle (mmap)
        self.db.take();

        // Read the working file and encrypt to persistent location
        let plaintext = std::fs::read(&self.working_path)?;
        crypto::file::write_encrypted_file(&self.encrypted_path, &plaintext, &self.master_key)?;

        // Reopen the database for continued use
        self.db = Some(redb::Database::open(&self.working_path)?);

        Ok(())
    }

    /// Close the database, encrypt to persistent storage, and clean up.
    ///
    /// After this call, the EncryptedDb is consumed and the working file is removed.
    pub fn close(mut self) -> Result<(), SoulError> {
        // Drop the database to release the file handle and all locks
        drop(self.db.take());

        // On Windows, file locks may not release immediately. Give a brief moment
        // for the OS to release the lock before we try to read the file.
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Read and encrypt to persistent location
        let plaintext = std::fs::read(&self.working_path)?;
        crypto::file::write_encrypted_file(&self.encrypted_path, &plaintext, &self.master_key)?;

        // Clean up working file
        let _ = std::fs::remove_file(&self.working_path);

        // Mark as closed so Drop doesn't try again
        self.encrypted_path = PathBuf::new();

        Ok(())
    }

    /// Returns the path to the persistent encrypted file.
    pub fn encrypted_path(&self) -> &Path {
        &self.encrypted_path
    }
}

impl Drop for EncryptedDb {
    fn drop(&mut self) {
        // If close() was already called, encrypted_path is empty — skip.
        if self.encrypted_path.as_os_str().is_empty() {
            return;
        }

        // Database wasn't properly closed via close() — best-effort encrypt.
        self.db.take();
        if let Ok(plaintext) = std::fs::read(&self.working_path) {
            let _ = crypto::file::write_encrypted_file(
                &self.encrypted_path,
                &plaintext,
                &self.master_key,
            );
        }
        let _ = std::fs::remove_file(&self.working_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TABLE: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("test_data");

    fn test_master_key() -> MasterKey {
        MasterKey::from_bytes([42u8; 32])
    }

    #[test]
    fn create_new_encrypted_db_and_write() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_path = dir.path().join("soul.redb.enc");
        let key = test_master_key();

        let db = EncryptedDb::open(&encrypted_path, &key).expect("should open");

        // Write some data
        {
            let write_txn = db
                .db()
                .expect("db should be open")
                .begin_write()
                .expect("should begin write");
            {
                let mut table = write_txn.open_table(TEST_TABLE).expect("should open table");
                table
                    .insert("hello", b"world".as_slice())
                    .expect("should insert");
            }
            write_txn.commit().expect("should commit");
        }

        // Close and encrypt
        db.close().expect("should close");

        // Verify the encrypted file exists and the working file is gone
        assert!(encrypted_path.exists());
        assert!(!dir.path().join("soul.redb.working").exists());
    }

    #[test]
    fn reopen_encrypted_db_reads_persisted_data() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_path = dir.path().join("soul.redb.enc");
        let key = test_master_key();

        // Create and write
        {
            let db = EncryptedDb::open(&encrypted_path, &key).expect("should open");
            let write_txn = db
                .db()
                .expect("db should be open")
                .begin_write()
                .expect("should begin write");
            {
                let mut table = write_txn.open_table(TEST_TABLE).expect("should open table");
                table
                    .insert("persist_key", b"persist_value".as_slice())
                    .expect("should insert");
            }
            write_txn.commit().expect("should commit");
            db.close().expect("should close");
        }

        // Reopen and read
        {
            let db = EncryptedDb::open(&encrypted_path, &key).expect("should reopen");
            {
                let read_txn = db
                    .db()
                    .expect("db should be open")
                    .begin_read()
                    .expect("should begin read");
                let table = read_txn.open_table(TEST_TABLE).expect("should open table");
                let value = table
                    .get("persist_key")
                    .expect("should get")
                    .expect("value should exist");
                assert_eq!(value.value(), b"persist_value");
            }
            db.close().expect("should close");
        }
    }

    #[test]
    fn encrypted_file_contains_no_plaintext() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_path = dir.path().join("soul.redb.enc");
        let key = test_master_key();

        let db = EncryptedDb::open(&encrypted_path, &key).expect("should open");
        let write_txn = db
            .db()
            .expect("db should be open")
            .begin_write()
            .expect("should begin write");
        {
            let mut table = write_txn.open_table(TEST_TABLE).expect("should open table");
            table
                .insert("secret_key", b"SUPER_SECRET_VALUE_12345".as_slice())
                .expect("should insert");
        }
        write_txn.commit().expect("should commit");
        db.close().expect("should close");

        // Read raw encrypted file and verify plaintext is not present
        let raw_bytes = std::fs::read(&encrypted_path).expect("should read encrypted file");
        let raw_str = String::from_utf8_lossy(&raw_bytes);
        assert!(
            !raw_str.contains("SUPER_SECRET_VALUE_12345"),
            "encrypted file must not contain plaintext values"
        );
        assert!(
            !raw_str.contains("secret_key"),
            "encrypted file must not contain plaintext keys"
        );
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_path = dir.path().join("soul.redb.enc");
        let key = test_master_key();
        let wrong_key = MasterKey::from_bytes([99u8; 32]);

        // Create and close with correct key
        {
            let db = EncryptedDb::open(&encrypted_path, &key).expect("should open");
            let write_txn = db
                .db()
                .expect("db should be open")
                .begin_write()
                .expect("should begin write");
            {
                let mut table = write_txn.open_table(TEST_TABLE).expect("should open table");
                table.insert("k", b"v".as_slice()).expect("should insert");
            }
            write_txn.commit().expect("should commit");
            db.close().expect("should close");
        }

        // Try to open with wrong key — should fail
        let result = EncryptedDb::open(&encrypted_path, &wrong_key);
        assert!(result.is_err(), "opening with wrong key must fail");
    }

    #[test]
    fn flush_persists_intermediate_state() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_path = dir.path().join("soul.redb.enc");
        let key = test_master_key();

        let mut db = EncryptedDb::open(&encrypted_path, &key).expect("should open");

        // Write and flush (without closing)
        {
            let write_txn = db
                .db()
                .expect("db should be open")
                .begin_write()
                .expect("should begin write");
            {
                let mut table = write_txn.open_table(TEST_TABLE).expect("should open table");
                table
                    .insert("flushed", b"yes".as_slice())
                    .expect("should insert");
            }
            write_txn.commit().expect("should commit");
        }
        db.flush().expect("should flush");

        // The encrypted file should now exist
        assert!(encrypted_path.exists());

        // Drop the current db (simulating crash after flush)
        drop(db);

        // Reopen from the flushed encrypted file
        let db2 = EncryptedDb::open(&encrypted_path, &key).expect("should reopen");
        {
            let read_txn = db2
                .db()
                .expect("db should be open")
                .begin_read()
                .expect("should begin read");
            let table = read_txn.open_table(TEST_TABLE).expect("should open table");
            let value = table
                .get("flushed")
                .expect("should get")
                .expect("value should exist");
            assert_eq!(value.value(), b"yes");
        }
        db2.close().expect("should close");
    }

    #[test]
    fn stale_working_file_cleaned_on_open() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_path = dir.path().join("soul.redb.enc");
        let working_path = dir.path().join("soul.redb.working");
        let key = test_master_key();

        // Simulate a stale working file from a crash
        std::fs::write(&working_path, b"stale garbage data").expect("should write stale file");
        assert!(working_path.exists());

        // Open should succeed (creates fresh db since no encrypted file exists)
        let db = EncryptedDb::open(&encrypted_path, &key).expect("should open despite stale file");
        db.close().expect("should close");
    }
}
