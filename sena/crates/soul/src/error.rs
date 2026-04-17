//! Soul subsystem error types.

use crypto::CryptoError;

#[derive(Debug, thiserror::Error)]
pub enum SoulError {
    /// Database operation failed.
    #[error("database error: {0}")]
    Database(String),

    /// Operation not yet implemented.
    #[error("not implemented: {0}")]
    NotImplemented(String),

    /// Encryption/decryption error from crypto crate.
    #[error("encryption error: {0}")]
    CryptoError(#[from] CryptoError),

    /// Encryption/decryption error message.
    #[error("encryption error: {0}")]
    Encryption(String),

    /// Encryption not initialized.
    #[error("encryption not initialized")]
    EncryptionNotInitialized,

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid input.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Store not initialized.
    #[error("store not initialized")]
    StoreNotInitialized,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soul_error_displays_correctly() {
        let err = SoulError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file missing",
        ));
        assert!(err.to_string().contains("file missing"));
    }

    #[test]
    fn soul_error_converts_from_crypto_error() {
        let crypto_err = CryptoError::Decryption("bad key".to_string());
        let soul_err: SoulError = crypto_err.into();
        assert!(soul_err.to_string().contains("bad key"));
    }
}
