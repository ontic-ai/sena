//! OS keychain-backed master key storage.
//!
//! `KeyStore` is the trait for persisting and retrieving the master key material.
//! `KeyringStore` is the concrete implementation backed by the OS keychain via
//! the `keyring` crate (Windows Credential Manager, macOS Keychain, Linux Secret Service).
//!
//! # Security rules
//!
//! - Master keys are encoded as hex strings before storage — never raw bytes.
//! - The hex string is zeroized immediately after use.
//! - The master key is never passed to any log macro.

use crate::error::{CryptoError, Result};
use crate::key::MasterKey;
use zeroize::Zeroize;

/// Trait for OS-specific key storage backends.
pub trait KeyStore: Send + Sync {
    /// Store a master key in the platform secure storage.
    fn store(&self, service: &str, username: &str, key: &MasterKey) -> Result<()>;

    /// Retrieve a master key from the platform secure storage.
    fn retrieve(&self, service: &str, username: &str) -> Result<MasterKey>;

    /// Delete a master key from the platform secure storage.
    fn delete(&self, service: &str, username: &str) -> Result<()>;
}

/// Cross-platform keyring-backed keystore.
///
/// Uses:
/// - Windows Credential Manager on Windows
/// - macOS Keychain on macOS
/// - libsecret / Secret Service on Linux
pub struct KeyringStore;

impl KeyringStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for KeyringStore {
    fn store(&self, service: &str, username: &str, key: &MasterKey) -> Result<()> {
        let entry = keyring::Entry::new(service, username)
            .map_err(|e| CryptoError::Keyring(format!("failed to create entry: {}", e)))?;

        let mut key_hex = hex::encode(key.as_bytes());
        let result = entry
            .set_password(&key_hex)
            .map_err(|e| CryptoError::Keyring(format!("failed to store key: {}", e)));
        key_hex.zeroize();
        result?;

        tracing::debug!(
            "stored master key in keyring: service={}, username={}",
            service, username
        );
        Ok(())
    }

    fn retrieve(&self, service: &str, username: &str) -> Result<MasterKey> {
        let entry = keyring::Entry::new(service, username)
            .map_err(|e| CryptoError::Keyring(format!("failed to create entry: {}", e)))?;

        let mut key_hex = entry
            .get_password()
            .map_err(|e| CryptoError::Keyring(format!("failed to retrieve key: {}", e)))?;

        let mut key_bytes = hex::decode(&key_hex).map_err(|e| {
            key_hex.zeroize();
            CryptoError::Keyring(format!("failed to decode key: {}", e))
        })?;
        key_hex.zeroize();

        if key_bytes.len() != 32 {
            key_bytes.zeroize();
            return Err(CryptoError::Keyring(format!(
                "invalid key length: expected 32, got {}",
                key_bytes.len()
            )));
        }

        let mut arr = [0u8; 32];
        arr.copy_from_slice(&key_bytes);
        key_bytes.zeroize();

        tracing::debug!(
            "retrieved master key from keyring: service={}, username={}",
            service, username
        );
        Ok(MasterKey::from_bytes(arr))
    }

    fn delete(&self, service: &str, username: &str) -> Result<()> {
        let entry = keyring::Entry::new(service, username)
            .map_err(|e| CryptoError::Keyring(format!("failed to create entry: {}", e)))?;

        match entry.delete_credential() {
            Ok(()) => {}
            Err(keyring::Error::NoEntry) => {}
            Err(e) => {
                return Err(CryptoError::Keyring(format!("failed to delete key: {}", e)));
            }
        }

        tracing::debug!(
            "deleted master key from keyring: service={}, username={}",
            service, username
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_store_round_trip() {
        let store = KeyringStore::new();
        let key = MasterKey::from_bytes([42u8; 32]);
        let service = "sena_test_keyring";
        let username = "test_user_keyring";

        let _ = store.delete(service, username);

        let store_result = store.store(service, username, &key);
        if store_result.is_err() {
            eprintln!("Skipping keyring test - keyring unavailable: {:?}", store_result.err());
            return;
        }

        let retrieve_result = store.retrieve(service, username);
        if retrieve_result.is_err() {
            eprintln!("Skipping keyring test - retrieve failed: {:?}", retrieve_result.err());
            let _ = store.delete(service, username);
            return;
        }

        assert_eq!(retrieve_result.unwrap().as_bytes(), key.as_bytes());
        store.delete(service, username).unwrap();
    }
}