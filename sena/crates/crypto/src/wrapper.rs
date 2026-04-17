use crate::blob::EncryptedBlob;
use crate::error::{CryptoError, Result};
use crate::key::Dek;
use crate::layer::EncryptionLayer;
use std::path::Path;

/// Wrapper for encrypted database access.
/// Provides encryption/decryption for database values using DEK.
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

    /// Encrypt plaintext data and return an EncryptedBlob.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedBlob> {
        let dek = self.dek.as_ref().ok_or(CryptoError::DekNotInitialized)?;
        self.layer.encrypt(dek, plaintext)
    }

    /// Decrypt an EncryptedBlob and return plaintext.
    pub fn decrypt(&self, blob: &EncryptedBlob) -> Result<Vec<u8>> {
        let dek = self.dek.as_ref().ok_or(CryptoError::DekNotInitialized)?;
        self.layer.decrypt(dek, blob)
    }

    /// Get the database path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Check if the wrapper is initialized with a DEK.
    pub fn is_initialized(&self) -> bool {
        self.dek.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{RealEncryptionLayer, StubEncryptionLayer};

    #[test]
    fn encrypted_db_construction() {
        let layer = StubEncryptionLayer::new();
        let db = EncryptedDb::new("/tmp/test.db", layer);
        assert_eq!(db.path(), Path::new("/tmp/test.db"));
        assert!(!db.is_initialized());
    }

    #[test]
    fn encrypted_db_init_succeeds() {
        let layer = StubEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test.db", layer);
        let dek = Dek::from_bytes([10u8; 32]);
        let result = db.init(dek);
        assert!(result.is_ok());
        assert!(db.is_initialized());
    }

    #[test]
    fn encrypted_db_encrypt_without_init_fails() {
        let layer = StubEncryptionLayer::new();
        let db = EncryptedDb::new("/tmp/test.db", layer);
        let result = db.encrypt(b"plaintext");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CryptoError::DekNotInitialized
        ));
    }

    #[test]
    fn encrypted_db_decrypt_without_init_fails() {
        let layer = StubEncryptionLayer::new();
        let db = EncryptedDb::new("/tmp/test.db", layer);
        let blob = EncryptedBlob::new([0u8; 12], vec![1, 2, 3]);
        let result = db.decrypt(&blob);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CryptoError::DekNotInitialized
        ));
    }

    #[test]
    fn encrypted_db_encrypt_after_init_succeeds() {
        let layer = StubEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test.db", layer);
        let dek = Dek::from_bytes([11u8; 32]);
        db.init(dek).unwrap();
        let result = db.encrypt(b"plaintext");
        assert!(result.is_ok());
    }

    #[test]
    fn encrypted_db_decrypt_after_init_succeeds() {
        let layer = StubEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test.db", layer);
        let dek = Dek::from_bytes([12u8; 32]);
        db.init(dek).unwrap();
        let blob = EncryptedBlob::new([0u8; 12], b"ciphertext".to_vec());
        let result = db.decrypt(&blob);
        assert!(result.is_ok());
    }

    #[test]
    fn encrypted_db_round_trip_with_stub() {
        let layer = StubEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test.db", layer);
        let dek = Dek::from_bytes([13u8; 32]);
        db.init(dek).unwrap();

        let original = b"test data for round trip";
        let blob = db.encrypt(original).unwrap();
        let decrypted = db.decrypt(&blob).unwrap();

        assert_eq!(&decrypted, original);
    }

    #[test]
    fn encrypted_db_round_trip_with_real_layer() {
        let layer = RealEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test_real.db", layer);
        let dek = Dek::from_bytes([99u8; 32]);
        db.init(dek).unwrap();

        let original = b"secret message with real encryption";
        let blob = db.encrypt(original).unwrap();

        // Ensure encrypted data is different from plaintext
        assert_ne!(blob.ciphertext(), original);

        let decrypted = db.decrypt(&blob).unwrap();
        assert_eq!(&decrypted, original);
    }

    #[test]
    fn encrypted_db_multiple_encryptions_produce_different_blobs() {
        let layer = RealEncryptionLayer::new();
        let mut db = EncryptedDb::new("/tmp/test_multi.db", layer);
        let dek = Dek::from_bytes([77u8; 32]);
        db.init(dek).unwrap();

        let plaintext = b"same plaintext";
        let blob1 = db.encrypt(plaintext).unwrap();
        let blob2 = db.encrypt(plaintext).unwrap();

        // Different nonces should produce different ciphertexts
        assert_ne!(blob1.nonce(), blob2.nonce());
        assert_ne!(blob1.ciphertext(), blob2.ciphertext());

        // Both should decrypt correctly
        assert_eq!(&db.decrypt(&blob1).unwrap(), plaintext);
        assert_eq!(&db.decrypt(&blob2).unwrap(), plaintext);
    }
}
