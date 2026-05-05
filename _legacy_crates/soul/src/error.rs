use crypto::CryptoError;

#[derive(Debug, thiserror::Error)]
pub enum SoulError {
    #[error("database error: {0}")]
    Database(String),

    #[error("encryption error: {0}")]
    Encryption(#[from] CryptoError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<redb::DatabaseError> for SoulError {
    fn from(e: redb::DatabaseError) -> Self {
        Self::Database(e.to_string())
    }
}

impl From<redb::TableError> for SoulError {
    fn from(e: redb::TableError) -> Self {
        Self::Database(e.to_string())
    }
}

impl From<redb::StorageError> for SoulError {
    fn from(e: redb::StorageError) -> Self {
        Self::Database(e.to_string())
    }
}

impl From<redb::CommitError> for SoulError {
    fn from(e: redb::CommitError) -> Self {
        Self::Database(e.to_string())
    }
}

impl From<redb::TransactionError> for SoulError {
    fn from(e: redb::TransactionError) -> Self {
        Self::Database(e.to_string())
    }
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
        let crypto_err = CryptoError::DecryptionFailed("bad key".to_string());
        let soul_err: SoulError = crypto_err.into();
        assert!(soul_err.to_string().contains("bad key"));
    }
}
