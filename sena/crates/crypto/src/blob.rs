//! AES-256-GCM encrypted blob type.
//!
//! `EncryptedBlob` is the portable container for encrypted data.
//! It holds a 12-byte nonce alongside the AES-GCM ciphertext (which includes
//! the 16-byte authentication tag). Both fields are required for decryption.

use serde::{Deserialize, Serialize};

/// An AES-256-GCM ciphertext blob with its associated nonce.
///
/// The nonce is generated fresh per-encryption call (`rand`).
/// The ciphertext includes the GCM authentication tag at the end.
///
/// # Security
///
/// - Nonces are NEVER reused. Each `EncryptedBlob` instance contains a unique nonce.
/// - The authentication tag makes tampering detectable during decryption.
/// - `EncryptedBlob` itself does not carry the key — the DEK is held separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedBlob {
    /// 12-byte random nonce used for this encryption operation.
    nonce: [u8; 12],
    /// AES-GCM ciphertext, including the 16-byte authentication tag.
    ciphertext: Vec<u8>,
}

impl EncryptedBlob {
    /// Construct a new blob from a nonce and ciphertext.
    ///
    /// Called by `RealEncryptionLayer::encrypt` after a successful AES-GCM
    /// encryption. Do not call this directly — use the encryption layer.
    pub fn new(nonce: [u8; 12], ciphertext: Vec<u8>) -> Self {
        Self { nonce, ciphertext }
    }

    /// The 12-byte nonce used for this blob.
    pub fn nonce(&self) -> &[u8; 12] {
        &self.nonce
    }

    /// The raw ciphertext bytes (including the GCM authentication tag).
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// Total encoded size in bytes (nonce + ciphertext).
    pub fn encoded_len(&self) -> usize {
        12 + self.ciphertext.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_stores_nonce_and_ciphertext() {
        let nonce = [1u8; 12];
        let ciphertext = vec![2u8, 3u8, 4u8];
        let blob = EncryptedBlob::new(nonce, ciphertext.clone());
        assert_eq!(blob.nonce(), &nonce);
        assert_eq!(blob.ciphertext(), ciphertext.as_slice());
    }

    #[test]
    fn blob_encoded_len_is_correct() {
        let blob = EncryptedBlob::new([0u8; 12], vec![0u8; 32]);
        assert_eq!(blob.encoded_len(), 44); // 12 + 32
    }

    #[test]
    fn blob_serializes_and_deserializes() {
        let nonce = [42u8; 12];
        let ciphertext = vec![10u8, 20u8, 30u8];
        let blob = EncryptedBlob::new(nonce, ciphertext);
        let json = serde_json::to_string(&blob).expect("serialize should succeed");
        let restored: EncryptedBlob =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(restored.nonce(), &nonce);
    }
}
