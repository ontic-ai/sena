use crate::error::Result;
use crate::key::{Dek, MasterKey, Passphrase};

/// Trait for encryption layer operations.
/// BONES phase: stub implementations only.
pub trait EncryptionLayer {
    /// Derive a master key from a passphrase using Argon2.
    fn derive_master_key(&self, passphrase: &Passphrase) -> Result<MasterKey>;

    /// Store the master key in the OS keyring.
    fn store_master_key(&self, key: &MasterKey) -> Result<()>;

    /// Retrieve the master key from the OS keyring.
    fn retrieve_master_key(&self) -> Result<MasterKey>;

    /// Derive a DEK from the master key.
    fn derive_dek(&self, master_key: &MasterKey) -> Result<Dek>;

    /// Encrypt plaintext bytes with the DEK.
    fn encrypt(&self, dek: &Dek, plaintext: &[u8]) -> Result<Vec<u8>>;

    /// Decrypt ciphertext bytes with the DEK.
    fn decrypt(&self, dek: &Dek, ciphertext: &[u8]) -> Result<Vec<u8>>;
}

/// Stub implementation for BONES phase.
pub struct StubEncryptionLayer;

impl StubEncryptionLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StubEncryptionLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl EncryptionLayer for StubEncryptionLayer {
    fn derive_master_key(&self, _passphrase: &Passphrase) -> Result<MasterKey> {
        tracing::trace!("derive_master_key: stub returning dummy key");
        Ok(MasterKey::from_bytes([0u8; 32]))
    }

    fn store_master_key(&self, _key: &MasterKey) -> Result<()> {
        tracing::trace!("store_master_key: stub no-op");
        Ok(())
    }

    fn retrieve_master_key(&self) -> Result<MasterKey> {
        tracing::trace!("retrieve_master_key: stub returning dummy key");
        Ok(MasterKey::from_bytes([0u8; 32]))
    }

    fn derive_dek(&self, _master_key: &MasterKey) -> Result<Dek> {
        tracing::trace!("derive_dek: stub returning dummy dek");
        Ok(Dek::from_bytes([0u8; 32]))
    }

    fn encrypt(&self, _dek: &Dek, plaintext: &[u8]) -> Result<Vec<u8>> {
        tracing::trace!("encrypt: stub passthrough");
        Ok(plaintext.to_vec())
    }

    fn decrypt(&self, _dek: &Dek, ciphertext: &[u8]) -> Result<Vec<u8>> {
        tracing::trace!("decrypt: stub passthrough");
        Ok(ciphertext.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_layer_derives_master_key() {
        let layer = StubEncryptionLayer::new();
        let passphrase = Passphrase::new("test".to_string());
        let result = layer.derive_master_key(&passphrase);
        assert!(result.is_ok());
    }

    #[test]
    fn stub_layer_stores_master_key() {
        let layer = StubEncryptionLayer::new();
        let key = MasterKey::from_bytes([1u8; 32]);
        let result = layer.store_master_key(&key);
        assert!(result.is_ok());
    }

    #[test]
    fn stub_layer_retrieves_master_key() {
        let layer = StubEncryptionLayer::new();
        let result = layer.retrieve_master_key();
        assert!(result.is_ok());
    }

    #[test]
    fn stub_layer_derives_dek() {
        let layer = StubEncryptionLayer::new();
        let master_key = MasterKey::from_bytes([2u8; 32]);
        let result = layer.derive_dek(&master_key);
        assert!(result.is_ok());
    }

    #[test]
    fn stub_layer_encrypts_data() {
        let layer = StubEncryptionLayer::new();
        let dek = Dek::from_bytes([3u8; 32]);
        let plaintext = b"test data";
        let result = layer.encrypt(&dek, plaintext);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), plaintext.to_vec());
    }

    #[test]
    fn stub_layer_decrypts_data() {
        let layer = StubEncryptionLayer::new();
        let dek = Dek::from_bytes([4u8; 32]);
        let ciphertext = b"test data";
        let result = layer.decrypt(&dek, ciphertext);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ciphertext.to_vec());
    }

    #[test]
    fn stub_layer_round_trip() {
        let layer = StubEncryptionLayer::new();
        let dek = Dek::from_bytes([5u8; 32]);
        let original = b"round trip test";

        let encrypted = layer.encrypt(&dek, original).unwrap();
        let decrypted = layer.decrypt(&dek, &encrypted).unwrap();

        assert_eq!(decrypted, original.to_vec());
    }
}
