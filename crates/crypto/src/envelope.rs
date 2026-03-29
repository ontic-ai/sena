//! Envelope encryption: generate, wrap, and unwrap Data Encryption Keys.

use rand::RngCore;

use crate::aes;
use crate::error::CryptoError;
use crate::keys::{MasterKey, DEK};

/// Wrapped (encrypted) DEK — safe to store on disk.
pub struct WrappedDEK(Vec<u8>);

impl WrappedDEK {
    /// Get the raw encrypted bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Create from raw encrypted bytes (e.g., read from disk).
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for WrappedDEK {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WrappedDEK([{} bytes])", self.0.len())
    }
}

/// Generate a new random DEK.
pub fn generate_dek() -> DEK {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    DEK::from_bytes(bytes)
}

/// Wrap (encrypt) a DEK using the master key.
///
/// The master key is used as the AES-256-GCM encryption key.
pub fn wrap_dek(dek: &DEK, master_key: &MasterKey) -> Result<WrappedDEK, CryptoError> {
    // Use master key bytes as a DEK for the AES operation
    let wrapping_key = DEK::from_bytes(*master_key.as_bytes());
    let encrypted = aes::encrypt(dek.as_bytes(), &wrapping_key)?;
    Ok(WrappedDEK(encrypted))
}

/// Unwrap (decrypt) a DEK using the master key.
pub fn unwrap_dek(wrapped: &WrappedDEK, master_key: &MasterKey) -> Result<DEK, CryptoError> {
    let wrapping_key = DEK::from_bytes(*master_key.as_bytes());
    let decrypted = aes::decrypt(wrapped.as_bytes(), &wrapping_key)?;

    if decrypted.len() != 32 {
        return Err(CryptoError::InvalidData(format!(
            "unwrapped DEK has invalid length: {} bytes, expected 32",
            decrypted.len()
        )));
    }

    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&decrypted);
    Ok(DEK::from_bytes(key_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_master_key() -> MasterKey {
        MasterKey::from_bytes([42u8; 32])
    }

    #[test]
    fn generate_dek_produces_unique_keys() {
        let dek1 = generate_dek();
        let dek2 = generate_dek();
        assert_ne!(dek1.as_bytes(), dek2.as_bytes());
    }

    #[test]
    fn wrap_unwrap_round_trip() {
        let master = test_master_key();
        let dek = generate_dek();
        let original_bytes = *dek.as_bytes();

        let wrapped = wrap_dek(&dek, &master).expect("wrap should succeed");
        let unwrapped = unwrap_dek(&wrapped, &master).expect("unwrap should succeed");

        assert_eq!(unwrapped.as_bytes(), &original_bytes);
    }

    #[test]
    fn unwrap_fails_with_wrong_master_key() {
        let master1 = MasterKey::from_bytes([1u8; 32]);
        let master2 = MasterKey::from_bytes([2u8; 32]);
        let dek = generate_dek();

        let wrapped = wrap_dek(&dek, &master1).expect("wrap");
        let result = unwrap_dek(&wrapped, &master2);
        assert!(result.is_err());
    }

    #[test]
    fn wrapped_dek_debug_does_not_leak() {
        let master = test_master_key();
        let dek = generate_dek();
        let wrapped = wrap_dek(&dek, &master).expect("wrap");
        let debug = format!("{:?}", wrapped);
        assert!(debug.starts_with("WrappedDEK(["));
        assert!(debug.contains("bytes]"));
    }
}
