/// Errors from the encryption layer.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("failed to derive key from passphrase: {0}")]
    KeyDerivation(String),

    #[error("failed to access OS keyring: {0}")]
    Keyring(String),

    #[error("encryption operation failed: {0}")]
    Encryption(String),

    #[error("decryption operation failed: {0}")]
    Decryption(String),

    #[error("nonce generation failed: {0}")]
    NonceGeneration(String),

    #[error("master key not initialized")]
    MasterKeyNotInitialized,

    #[error("DEK not initialized")]
    DekNotInitialized,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CryptoError>;
