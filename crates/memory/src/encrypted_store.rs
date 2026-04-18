use crate::error::MemoryError;
use crypto::working_file;
use crypto::MasterKey;
use std::path::{Path, PathBuf};

/// Manages encrypted storage for ech0's persistent files.
///
/// ech0 stores multiple files (graph redb, vector index). This wrapper
/// provides a directory-level encryption layer:
///
/// - On open: all `.enc` files in the encrypted directory are decrypted
///   to a working directory where ech0 can operate normally.
/// - On flush/close: all files in the working directory are encrypted
///   back to the encrypted directory.
///
/// The working directory is cleaned up on close or drop.
pub struct EncryptedStore {
    /// Persistent directory containing encrypted files.
    encrypted_dir: PathBuf,
    /// Temporary working directory with decrypted files for ech0.
    working_dir: PathBuf,
    /// Master key for envelope encryption.
    master_key: MasterKey,
    /// Whether the store has been properly closed.
    closed: bool,
}

impl EncryptedStore {
    /// Open or create an encrypted store.
    ///
    /// If `encrypted_dir` contains `.enc` files, they are decrypted to a
    /// working directory inside `encrypted_dir`. If no encrypted files exist
    /// (first boot), an empty working directory is created.
    pub fn open(encrypted_dir: &Path, master_key: &MasterKey) -> Result<Self, MemoryError> {
        let working_dir = encrypted_dir.join(".working");

        // Create directories if they don't exist
        std::fs::create_dir_all(encrypted_dir)?;

        // Clean up any stale working directory from a previous crash
        working_file::clean_stale_working(&working_dir)?;
        std::fs::create_dir_all(&working_dir)?;

        // Decrypt all .enc files to the working directory
        for entry in std::fs::read_dir(encrypted_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) == Some("enc") {
                let stem = path
                    .file_stem()
                    .ok_or_else(|| MemoryError::Store("invalid encrypted filename".to_string()))?;

                // Validate filename has no path separators (directory traversal protection)
                let stem_str = stem.to_string_lossy();
                if stem_str.contains('/') || stem_str.contains('\\') || stem_str.contains("..") {
                    return Err(MemoryError::Store(
                        "encrypted filename contains path separator".to_string(),
                    ));
                }

                let working_file_path = working_dir.join(stem);
                working_file::decrypt_to_working(&path, &working_file_path, master_key)?;
            }
        }

        let owned_key = MasterKey::from_bytes(*master_key.as_bytes());

        Ok(Self {
            encrypted_dir: encrypted_dir.to_path_buf(),
            working_dir,
            master_key: owned_key,
            closed: false,
        })
    }

    /// Returns the path to the working directory where ech0 should
    /// store its files. All files created here will be encrypted on
    /// flush/close.
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Encrypt all files in the working directory to the encrypted directory.
    ///
    /// Existing `.enc` files are overwritten. New files get `.enc` appended.
    pub fn flush(&self) -> Result<(), MemoryError> {
        self.encrypt_working_files()
    }

    /// Close the store: encrypt all working files and clean up.
    pub fn close(mut self) -> Result<(), MemoryError> {
        self.encrypt_working_files()?;

        working_file::cleanup_working(&self.working_dir);
        self.closed = true;
        Ok(())
    }

    /// Returns the path to the persistent encrypted directory.
    pub fn encrypted_dir(&self) -> &Path {
        &self.encrypted_dir
    }

    fn encrypt_working_files(&self) -> Result<(), MemoryError> {
        for entry in std::fs::read_dir(&self.working_dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let file_name = path
                .file_name()
                .ok_or_else(|| MemoryError::Store("invalid working filename".to_string()))?;

            let mut enc_name = file_name.to_os_string();
            enc_name.push(".enc");
            let enc_path = self.encrypted_dir.join(enc_name);

            working_file::encrypt_from_working(&path, &enc_path, &self.master_key)?;
        }
        Ok(())
    }
}

impl Drop for EncryptedStore {
    fn drop(&mut self) {
        if self.closed {
            return;
        }

        // Best-effort encrypt on drop if close() wasn't called
        let _ = self.encrypt_working_files();
        working_file::cleanup_working(&self.working_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_master_key() -> MasterKey {
        MasterKey::from_bytes([42u8; 32])
    }

    #[test]
    fn create_new_store_and_add_files() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_dir = dir.path().join("memory_store");
        let key = test_master_key();

        let store = EncryptedStore::open(&encrypted_dir, &key).expect("should open");

        // Write files to the working directory (simulating ech0 operations)
        std::fs::write(store.working_dir().join("graph.redb"), b"graph data")
            .expect("should write graph");
        std::fs::write(store.working_dir().join("vectors.idx"), b"vector data")
            .expect("should write vectors");

        store.close().expect("should close");

        // Verify encrypted files exist
        assert!(encrypted_dir.join("graph.redb.enc").exists());
        assert!(encrypted_dir.join("vectors.idx.enc").exists());
        // Working dir should be cleaned up
        assert!(!encrypted_dir.join(".working").exists());
    }

    #[test]
    fn reopen_store_recovers_files() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_dir = dir.path().join("memory_store");
        let key = test_master_key();

        // Create and populate
        {
            let store = EncryptedStore::open(&encrypted_dir, &key).expect("should open");
            std::fs::write(store.working_dir().join("graph.redb"), b"persisted graph")
                .expect("should write");
            store.close().expect("should close");
        }

        // Reopen and verify
        {
            let store = EncryptedStore::open(&encrypted_dir, &key).expect("should reopen");
            let content = std::fs::read(store.working_dir().join("graph.redb"))
                .expect("should read recovered file");
            assert_eq!(content, b"persisted graph");
            store.close().expect("should close");
        }
    }

    #[test]
    fn encrypted_files_contain_no_plaintext() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_dir = dir.path().join("memory_store");
        let key = test_master_key();

        let store = EncryptedStore::open(&encrypted_dir, &key).expect("should open");
        std::fs::write(
            store.working_dir().join("graph.redb"),
            b"HIGHLY_SENSITIVE_MEMORY_CONTENT",
        )
        .expect("should write");
        store.close().expect("should close");

        // Read raw encrypted file
        let raw = std::fs::read(encrypted_dir.join("graph.redb.enc"))
            .expect("should read encrypted file");
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(
            !raw_str.contains("HIGHLY_SENSITIVE_MEMORY_CONTENT"),
            "encrypted file must not contain plaintext"
        );
    }

    #[test]
    fn wrong_key_fails_to_reopen() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_dir = dir.path().join("memory_store");
        let key = test_master_key();
        let wrong_key = MasterKey::from_bytes([99u8; 32]);

        // Create with correct key
        {
            let store = EncryptedStore::open(&encrypted_dir, &key).expect("should open");
            std::fs::write(store.working_dir().join("data.bin"), b"secret").expect("should write");
            store.close().expect("should close");
        }

        // Try with wrong key
        let result = EncryptedStore::open(&encrypted_dir, &wrong_key);
        assert!(result.is_err(), "opening with wrong key must fail");
    }

    #[test]
    fn flush_persists_without_closing() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_dir = dir.path().join("memory_store");
        let key = test_master_key();

        let store = EncryptedStore::open(&encrypted_dir, &key).expect("should open");

        std::fs::write(store.working_dir().join("data.bin"), b"flush test").expect("should write");
        store.flush().expect("should flush");

        // Encrypted file should exist while store is still open
        assert!(encrypted_dir.join("data.bin.enc").exists());

        // Drop without close — drop handler does best-effort cleanup
        drop(store);

        // Reopen and verify
        let store2 = EncryptedStore::open(&encrypted_dir, &key).expect("should reopen");
        let content = std::fs::read(store2.working_dir().join("data.bin")).expect("should read");
        assert_eq!(content, b"flush test");
        store2.close().expect("should close");
    }

    #[test]
    fn stale_working_dir_cleaned_on_open() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let encrypted_dir = dir.path().join("memory_store");
        let working_dir = encrypted_dir.join(".working");
        let key = test_master_key();

        // Create encrypted dir and stale working dir
        std::fs::create_dir_all(&working_dir).expect("should create dirs");
        std::fs::write(working_dir.join("stale.bin"), b"stale crash data")
            .expect("should write stale file");

        // Open should succeed and clean up stale data
        let store = EncryptedStore::open(&encrypted_dir, &key).expect("should open");
        // Stale file should be gone
        assert!(!store.working_dir().join("stale.bin").exists());
        store.close().expect("should close");
    }
}
