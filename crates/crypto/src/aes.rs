//! AES-256-GCM encryption and decryption.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;

use crate::error::CryptoError;
use crate::keys::DEK;

const NONCE_LEN: usize = 12;

/// Encrypt plaintext with AES-256-GCM using a fresh random nonce.
///
/// Returns `nonce (12 bytes) || ciphertext`.
/// A new random nonce is generated for every call.
pub fn encrypt(plaintext: &[u8], key: &DEK) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

    let mut output = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt data that was produced by [`encrypt`].
///
/// Expects `nonce (12 bytes) || ciphertext`.
pub fn decrypt(data: &[u8], key: &DEK) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN {
        return Err(CryptoError::InvalidData(format!(
            "data too short for nonce: {} bytes, need at least {}",
            data.len(),
            NONCE_LEN
        )));
    }

    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> DEK {
        DEK::from_bytes([1u8; 32])
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = test_key();
        let plaintext = b"hello sena encryption";
        let encrypted = encrypt(plaintext, &key).expect("encrypt should succeed");
        let decrypted = decrypt(&encrypted, &key).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_produces_different_nonces() {
        let key = test_key();
        let plaintext = b"same data";
        let enc1 = encrypt(plaintext, &key).expect("first encrypt");
        let enc2 = encrypt(plaintext, &key).expect("second encrypt");
        // First 12 bytes are the nonce — they must differ
        assert_ne!(&enc1[..NONCE_LEN], &enc2[..NONCE_LEN]);
    }

    #[test]
    fn decrypt_fails_with_wrong_key() {
        let key1 = DEK::from_bytes([1u8; 32]);
        let key2 = DEK::from_bytes([2u8; 32]);
        let plaintext = b"secret";
        let encrypted = encrypt(plaintext, &key1).expect("encrypt");
        let result = decrypt(&encrypted, &key2);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_fails_with_short_data() {
        let key = test_key();
        let result = decrypt(&[0u8; 5], &key);
        assert!(result.is_err());
        if let Err(CryptoError::InvalidData(msg)) = result {
            assert!(msg.contains("too short"));
        } else {
            panic!("expected InvalidData error");
        }
    }

    #[test]
    fn encrypt_empty_plaintext() {
        let key = test_key();
        let encrypted = encrypt(b"", &key).expect("encrypt empty");
        let decrypted = decrypt(&encrypted, &key).expect("decrypt empty");
        assert!(decrypted.is_empty());
    }
}
