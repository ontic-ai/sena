use crate::blob::EncryptedBlob;
use crate::error::{CryptoError, Result};
use crate::key::{Dek, MasterKey, Passphrase};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use hkdf::Hkdf;
use sha2::Sha256;

/// Trait for encryption layer operations.
///
/// Must be Send + Sync to allow sharing across async tasks.
pub trait EncryptionLayer: Send + Sync {
    /// Derive a master key from a passphrase using Argon2.
    fn derive_master_key(
        &self,
        passphrase: &Passphrase,
        salt: Option<&[u8]>,
    ) -> Result<(MasterKey, Vec<u8>)>;

    /// Derive a DEK from the master key using HKDF.
    fn derive_dek(&self, master_key: &MasterKey, context: &[u8]) -> Result<Dek>;

    /// Encrypt plaintext bytes with the DEK and return an EncryptedBlob.
    fn encrypt(&self, dek: &Dek, plaintext: &[u8]) -> Result<EncryptedBlob>;

    /// Decrypt an EncryptedBlob with the DEK.
    fn decrypt(&self, dek: &Dek, blob: &EncryptedBlob) -> Result<Vec<u8>>;
}

/// Real encryption layer implementation using AES-256-GCM and Argon2.
pub struct RealEncryptionLayer;

impl RealEncryptionLayer {
    pub fn new() -> Self {
        Self
    }

    /// Generate a random salt for Argon2.
    fn generate_salt(&self) -> Result<SaltString> {
        // Use getrandom to get random bytes compatible with password-hash's expectations
        let mut salt_bytes = [0u8; 16];
        getrandom::getrandom(&mut salt_bytes)
            .map_err(|e| CryptoError::KeyDerivation(format!("random generation failed: {}", e)))?;

        SaltString::encode_b64(&salt_bytes)
            .map_err(|e| CryptoError::KeyDerivation(format!("salt encoding failed: {}", e)))
    }

    /// Generate a random nonce for AES-GCM.
    fn generate_nonce(&self) -> Result<[u8; 12]> {
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut nonce).map_err(|e| {
            CryptoError::NonceGeneration(format!("random generation failed: {}", e))
        })?;
        Ok(nonce)
    }
}

impl Default for RealEncryptionLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl EncryptionLayer for RealEncryptionLayer {
    fn derive_master_key(
        &self,
        passphrase: &Passphrase,
        salt: Option<&[u8]>,
    ) -> Result<(MasterKey, Vec<u8>)> {
        let salt_string = if let Some(salt_bytes) = salt {
            // When salt is provided, treat it as the base64 string representation bytes
            let salt_str = std::str::from_utf8(salt_bytes)
                .map_err(|e| CryptoError::KeyDerivation(format!("invalid salt UTF-8: {}", e)))?;
            SaltString::from_b64(salt_str)
                .map_err(|e| CryptoError::KeyDerivation(format!("invalid salt string: {}", e)))?
        } else {
            self.generate_salt()?
        };

        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(passphrase.as_bytes(), &salt_string)
            .map_err(|e| CryptoError::KeyDerivation(format!("argon2 hashing failed: {}", e)))?;

        let hash_bytes = password_hash
            .hash
            .ok_or_else(|| CryptoError::KeyDerivation("no hash output".to_string()))?;

        if hash_bytes.len() < 32 {
            return Err(CryptoError::KeyDerivation(format!(
                "insufficient hash length: got {}, need 32",
                hash_bytes.len()
            )));
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&hash_bytes.as_bytes()[..32]);

        let salt_bytes = salt_string.as_str().as_bytes().to_vec();

        tracing::debug!("derived master key from passphrase using Argon2id");
        Ok((MasterKey::from_bytes(key_bytes), salt_bytes))
    }

    fn derive_dek(&self, master_key: &MasterKey, context: &[u8]) -> Result<Dek> {
        let hkdf = Hkdf::<Sha256>::new(None, master_key.as_bytes());
        let mut dek_bytes = [0u8; 32];
        hkdf.expand(context, &mut dek_bytes)
            .map_err(|e| CryptoError::KeyDerivation(format!("HKDF expansion failed: {}", e)))?;

        tracing::debug!("derived DEK from master key using HKDF-SHA256");
        Ok(Dek::from_bytes(dek_bytes))
    }

    fn encrypt(&self, dek: &Dek, plaintext: &[u8]) -> Result<EncryptedBlob> {
        let cipher = Aes256Gcm::new_from_slice(dek.as_bytes())
            .map_err(|e| CryptoError::Encryption(format!("cipher init failed: {}", e)))?;

        let nonce_bytes = self.generate_nonce()?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| CryptoError::Encryption(format!("AES-GCM encryption failed: {}", e)))?;

        tracing::trace!("encrypted {} bytes with AES-256-GCM", plaintext.len());
        Ok(EncryptedBlob::new(nonce_bytes, ciphertext))
    }

    fn decrypt(&self, dek: &Dek, blob: &EncryptedBlob) -> Result<Vec<u8>> {
        let cipher = Aes256Gcm::new_from_slice(dek.as_bytes())
            .map_err(|e| CryptoError::Decryption(format!("cipher init failed: {}", e)))?;

        let nonce = Nonce::from_slice(blob.nonce());

        let plaintext = cipher
            .decrypt(nonce, blob.ciphertext())
            .map_err(|e| CryptoError::Decryption(format!("AES-GCM decryption failed: {}", e)))?;

        tracing::trace!("decrypted {} bytes with AES-256-GCM", plaintext.len());
        Ok(plaintext)
    }
}

/// Stub implementation for testing without real crypto.
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
    fn derive_master_key(
        &self,
        _passphrase: &Passphrase,
        _salt: Option<&[u8]>,
    ) -> Result<(MasterKey, Vec<u8>)> {
        tracing::trace!("derive_master_key: stub returning dummy key");
        Ok((MasterKey::from_bytes([0u8; 32]), vec![0u8; 16]))
    }

    fn derive_dek(&self, _master_key: &MasterKey, _context: &[u8]) -> Result<Dek> {
        tracing::trace!("derive_dek: stub returning dummy dek");
        Ok(Dek::from_bytes([0u8; 32]))
    }

    fn encrypt(&self, _dek: &Dek, plaintext: &[u8]) -> Result<EncryptedBlob> {
        tracing::trace!("encrypt: stub passthrough");
        Ok(EncryptedBlob::new([0u8; 12], plaintext.to_vec()))
    }

    fn decrypt(&self, _dek: &Dek, blob: &EncryptedBlob) -> Result<Vec<u8>> {
        tracing::trace!("decrypt: stub passthrough");
        Ok(blob.ciphertext().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_layer_derives_master_key() {
        let layer = RealEncryptionLayer::new();
        let passphrase = Passphrase::new("test_password_123".to_string());
        let (key, salt) = layer.derive_master_key(&passphrase, None).unwrap();
        assert_eq!(key.as_bytes().len(), 32);
        assert!(!salt.is_empty());
    }

    #[test]
    fn real_layer_master_key_derivation_is_deterministic() {
        let layer = RealEncryptionLayer::new();
        let passphrase = Passphrase::new("same_password".to_string());

        let (key1, salt1) = layer.derive_master_key(&passphrase, None).unwrap();
        let (key2, _salt2) = layer.derive_master_key(&passphrase, Some(&salt1)).unwrap();

        assert_eq!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn real_layer_derives_dek() {
        let layer = RealEncryptionLayer::new();
        let master_key = MasterKey::from_bytes([42u8; 32]);
        let context = b"test_context";
        let dek = layer.derive_dek(&master_key, context).unwrap();
        assert_eq!(dek.as_bytes().len(), 32);
    }

    #[test]
    fn real_layer_dek_derivation_is_deterministic() {
        let layer = RealEncryptionLayer::new();
        let master_key = MasterKey::from_bytes([99u8; 32]);
        let context = b"same_context";

        let dek1 = layer.derive_dek(&master_key, context).unwrap();
        let dek2 = layer.derive_dek(&master_key, context).unwrap();

        assert_eq!(dek1.as_bytes(), dek2.as_bytes());
    }

    #[test]
    fn real_layer_dek_changes_with_context() {
        let layer = RealEncryptionLayer::new();
        let master_key = MasterKey::from_bytes([77u8; 32]);

        let dek1 = layer.derive_dek(&master_key, b"context1").unwrap();
        let dek2 = layer.derive_dek(&master_key, b"context2").unwrap();

        assert_ne!(dek1.as_bytes(), dek2.as_bytes());
    }

    #[test]
    fn real_layer_encrypts_and_decrypts() {
        let layer = RealEncryptionLayer::new();
        let dek = Dek::from_bytes([123u8; 32]);
        let plaintext = b"secret message";

        let blob = layer.encrypt(&dek, plaintext).unwrap();
        assert_ne!(blob.ciphertext(), plaintext);
        assert_eq!(blob.nonce().len(), 12);

        let decrypted = layer.decrypt(&dek, &blob).unwrap();
        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn real_layer_wrong_key_decryption_fails() {
        let layer = RealEncryptionLayer::new();
        let dek1 = Dek::from_bytes([1u8; 32]);
        let dek2 = Dek::from_bytes([2u8; 32]);
        let plaintext = b"secret";

        let blob = layer.encrypt(&dek1, plaintext).unwrap();
        let result = layer.decrypt(&dek2, &blob);

        assert!(result.is_err());
    }

    #[test]
    fn real_layer_encrypts_with_unique_nonces() {
        let layer = RealEncryptionLayer::new();
        let dek = Dek::from_bytes([55u8; 32]);
        let plaintext = b"same plaintext";

        let blob1 = layer.encrypt(&dek, plaintext).unwrap();
        let blob2 = layer.encrypt(&dek, plaintext).unwrap();

        // Nonces should be different
        assert_ne!(blob1.nonce(), blob2.nonce());
        // Ciphertexts should be different due to different nonces
        assert_ne!(blob1.ciphertext(), blob2.ciphertext());

        // But both should decrypt to the same plaintext
        let decrypted1 = layer.decrypt(&dek, &blob1).unwrap();
        let decrypted2 = layer.decrypt(&dek, &blob2).unwrap();
        assert_eq!(decrypted1, decrypted2);
        assert_eq!(&decrypted1, plaintext);
    }

    #[test]
    fn real_layer_round_trip_with_empty_plaintext() {
        let layer = RealEncryptionLayer::new();
        let dek = Dek::from_bytes([88u8; 32]);
        let plaintext = b"";

        let blob = layer.encrypt(&dek, plaintext).unwrap();
        let decrypted = layer.decrypt(&dek, &blob).unwrap();

        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn real_layer_round_trip_with_large_plaintext() {
        let layer = RealEncryptionLayer::new();
        let dek = Dek::from_bytes([66u8; 32]);
        let plaintext = vec![0xAB; 1024 * 10]; // 10 KB

        let blob = layer.encrypt(&dek, &plaintext).unwrap();
        let decrypted = layer.decrypt(&dek, &blob).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn stub_layer_derives_master_key() {
        let layer = StubEncryptionLayer::new();
        let passphrase = Passphrase::new("test".to_string());
        let (key, salt) = layer.derive_master_key(&passphrase, None).unwrap();
        assert_eq!(key.as_bytes(), &[0u8; 32]);
        assert_eq!(salt.len(), 16);
    }

    #[test]
    fn stub_layer_derives_dek() {
        let layer = StubEncryptionLayer::new();
        let master_key = MasterKey::from_bytes([2u8; 32]);
        let dek = layer.derive_dek(&master_key, b"context").unwrap();
        assert_eq!(dek.as_bytes(), &[0u8; 32]);
    }

    #[test]
    fn stub_layer_encrypts_data() {
        let layer = StubEncryptionLayer::new();
        let dek = Dek::from_bytes([3u8; 32]);
        let plaintext = b"test data";
        let blob = layer.encrypt(&dek, plaintext).unwrap();
        assert_eq!(blob.ciphertext(), plaintext);
    }

    #[test]
    fn stub_layer_decrypts_data() {
        let layer = StubEncryptionLayer::new();
        let dek = Dek::from_bytes([4u8; 32]);
        let blob = EncryptedBlob::new([0u8; 12], b"test data".to_vec());
        let decrypted = layer.decrypt(&dek, &blob).unwrap();
        assert_eq!(&decrypted, b"test data");
    }

    #[test]
    fn stub_layer_round_trip() {
        let layer = StubEncryptionLayer::new();
        let dek = Dek::from_bytes([5u8; 32]);
        let original = b"round trip test";

        let blob = layer.encrypt(&dek, original).unwrap();
        let decrypted = layer.decrypt(&dek, &blob).unwrap();

        assert_eq!(&decrypted, original);
    }
}
