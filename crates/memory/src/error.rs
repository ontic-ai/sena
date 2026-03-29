use crypto::CryptoError;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("encryption error: {0}")]
    Encryption(#[from] CryptoError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("store error: {0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_error_displays_correctly() {
        let err = MemoryError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file missing",
        ));
        assert!(err.to_string().contains("file missing"));
    }

    #[test]
    fn memory_error_converts_from_crypto_error() {
        let crypto_err = CryptoError::DecryptionFailed("bad key".to_string());
        let mem_err: MemoryError = crypto_err.into();
        assert!(mem_err.to_string().contains("bad key"));
    }
}
