//! Argon2id key derivation from user passphrase.

use rand::RngCore;
use zeroize::ZeroizeOnDrop;

use crate::error::CryptoError;
use crate::keys::MasterKey;

const SALT_LEN: usize = 16;

/// Cryptographic salt for Argon2id derivation.
/// Not secret — safe to store in plaintext alongside encrypted files.
#[derive(Clone)]
pub struct Salt([u8; SALT_LEN]);

impl Salt {
    /// Create a Salt from raw bytes.
    pub fn from_bytes(bytes: [u8; SALT_LEN]) -> Self {
        Self(bytes)
    }

    /// Get a reference to the salt bytes.
    pub fn as_bytes(&self) -> &[u8; SALT_LEN] {
        &self.0
    }
}

/// User passphrase — zeroed from memory immediately after key derivation.
#[derive(ZeroizeOnDrop)]
pub struct Passphrase(String);

impl Passphrase {
    /// Create a Passphrase from a string.
    pub fn new(passphrase: String) -> Self {
        Self(passphrase)
    }

    /// Get the passphrase as bytes.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Passphrase([REDACTED])")
    }
}

/// Generate a random salt for Argon2id derivation.
pub fn generate_salt() -> Salt {
    let mut bytes = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut bytes);
    Salt(bytes)
}

/// Derive a MasterKey from a passphrase using Argon2id.
///
/// Parameters per architecture.md §15.3:
/// - memory_cost: 64 MB (65536 KiB)
/// - time_cost: 3 iterations
/// - parallelism: 1
pub fn derive_master_key(passphrase: &Passphrase, salt: &Salt) -> Result<MasterKey, CryptoError> {
    use argon2::Argon2;

    let params = argon2::Params::new(65536, 3, 1, Some(32))
        .map_err(|e| CryptoError::KeyDerivationFailed(e.to_string()))?;

    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let mut output = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt.as_bytes(), &mut output)
        .map_err(|e| CryptoError::KeyDerivationFailed(e.to_string()))?;

    Ok(MasterKey::from_bytes(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_master_key_deterministic() {
        let passphrase = Passphrase::new("test-passphrase-123".to_string());
        let salt = Salt::from_bytes([0u8; SALT_LEN]);
        let key1 = derive_master_key(&passphrase, &salt).expect("derive 1");

        let passphrase2 = Passphrase::new("test-passphrase-123".to_string());
        let key2 = derive_master_key(&passphrase2, &salt).expect("derive 2");

        assert_eq!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn different_salts_produce_different_keys() {
        let passphrase1 = Passphrase::new("same-passphrase".to_string());
        let passphrase2 = Passphrase::new("same-passphrase".to_string());
        let salt1 = Salt::from_bytes([1u8; SALT_LEN]);
        let salt2 = Salt::from_bytes([2u8; SALT_LEN]);
        let key1 = derive_master_key(&passphrase1, &salt1).expect("derive 1");
        let key2 = derive_master_key(&passphrase2, &salt2).expect("derive 2");
        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn passphrase_debug_redacts() {
        let passphrase = Passphrase::new("super-secret".to_string());
        let debug_output = format!("{:?}", passphrase);
        assert_eq!(debug_output, "Passphrase([REDACTED])");
        assert!(!debug_output.contains("super-secret"));
    }

    #[test]
    fn generate_salt_produces_random_values() {
        let salt1 = generate_salt();
        let salt2 = generate_salt();
        assert_ne!(salt1.as_bytes(), salt2.as_bytes());
    }
}
