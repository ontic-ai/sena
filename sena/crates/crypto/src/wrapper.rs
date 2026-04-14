use crate::error::Result;
use crate::key::Dek;
use crate::layer::EncryptionLayer;
use std::path::Path;

/// Wrapper for encrypted database access.
/// BONES phase: stub implementation only.
pub struct EncryptedDb<L: EncryptionLayer> {
    path: std::path::PathBuf,
    layer: L,
    dek: Option<Dek>,
}

impl<L: EncryptionLayer> EncryptedDb<L> {
    /// Create a new encrypted database wrapper.
    pub fn new(path: impl AsRef<Path>, layer: L) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            layer,
            dek: None,
        }
    }

    /// Initialize the wrapper with a DEK.
    pub fn init(&mut self, dek: Dek) -> Result<()> {
        tracing::trace!("EncryptedDb::init at path: {:?}", self.path);
        self.dek = Some(dek);
        Ok(())
    }

    /// Read and decrypt data from the database.
    pub fn read(&self, key: &[u8]) -> Result<Vec<u8>> {
        tracing::trace!("EncryptedDb::read key length: {}", key.len());
        let dek = self
            .dek
            .as_ref()
            .ok_or(crate::error::CryptoError::DekNotInitialized)?;
        // BONES stub: return dummy data
        let dummy_ciphertext = b"stub_encrypted_data";
        self.layer.decrypt(dek, dummy_ciphertext)
    }

    /// Encrypt and write data to the database.
    pub fn write(&self, key: &[u8], value: &[u8]) -> Result<()> {
        tracing::trace!(
            "EncryptedDb::write key length: {}, value length: {}",
            key.len(),
            value.len()
        );
        let dek = self
            .dek
            .as_ref()
            .ok_or(crate::error::CryptoError::DekNotInitialized)?;
        let _encrypted = self.layer.encrypt(dek, value)?;
        // BONES stub: no actual write
        Ok(())
    }

    /// Get the database path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::StubEncryptionLayer;

    #[test]
    fn encrypted_db_construction() {
        let layer = StubEncryptionLayer::new();
        let db = EncryptedDb::new("/tmp/test.db", layer);
        assert_eq!(db.path(), Path::new("/tmp/test.db"));
    }

    #[test]
    fn encrypted_db_init_succeeds() {
        let layer = StubEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test.db", layer);
        let dek = Dek::from_bytes([10u8; 32]);
        let result = db.init(dek);
        assert!(result.is_ok());
    }

    #[test]
    fn encrypted_db_read_without_init_fails() {
        let layer = StubEncryptionLayer::new();
        let db = EncryptedDb::new("/tmp/test.db", layer);
        let result = db.read(b"key");
        assert!(result.is_err());
    }

    #[test]
    fn encrypted_db_write_without_init_fails() {
        let layer = StubEncryptionLayer::new();
        let db = EncryptedDb::new("/tmp/test.db", layer);
        let result = db.write(b"key", b"value");
        assert!(result.is_err());
    }

    #[test]
    fn encrypted_db_read_after_init_succeeds() {
        let layer = StubEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test.db", layer);
        let dek = Dek::from_bytes([11u8; 32]);
        db.init(dek).unwrap();
        let result = db.read(b"key");
        assert!(result.is_ok());
    }

    #[test]
    fn encrypted_db_write_after_init_succeeds() {
        let layer = StubEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test.db", layer);
        let dek = Dek::from_bytes([12u8; 32]);
        db.init(dek).unwrap();
        let result = db.write(b"key", b"value");
        assert!(result.is_ok());
    }
}
