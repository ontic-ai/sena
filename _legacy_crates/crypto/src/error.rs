use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("key derivation failed: {0}")]
    KeyDerivationFailed(String),

    #[error("keychain error: {0}")]
    KeychainError(String),

    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("invalid data: {0}")]
    InvalidData(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_encryption_failed() {
        let err = CryptoError::EncryptionFailed("test reason".to_string());
        assert_eq!(err.to_string(), "encryption failed: test reason");
    }

    #[test]
    fn error_display_decryption_failed() {
        let err = CryptoError::DecryptionFailed("bad nonce".to_string());
        assert_eq!(err.to_string(), "decryption failed: bad nonce");
    }

    #[test]
    fn error_display_key_derivation_failed() {
        let err = CryptoError::KeyDerivationFailed("argon2 error".to_string());
        assert_eq!(err.to_string(), "key derivation failed: argon2 error");
    }

    #[test]
    fn error_display_keychain_error() {
        let err = CryptoError::KeychainError("no keychain available".to_string());
        assert_eq!(err.to_string(), "keychain error: no keychain available");
    }

    #[test]
    fn error_display_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = CryptoError::IoError(io_err);
        assert_eq!(err.to_string(), "io error: file not found");
    }

    #[test]
    fn error_display_invalid_data() {
        let err = CryptoError::InvalidData("corrupted ciphertext".to_string());
        assert_eq!(err.to_string(), "invalid data: corrupted ciphertext");
    }
}
