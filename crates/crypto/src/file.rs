//! File-level encryption: read and write encrypted files on disk.
//!
//! File format: `[u32 WrappedDEK length][WrappedDEK bytes][nonce || ciphertext]`

use std::path::Path;

use crate::aes;
use crate::envelope::{generate_dek, unwrap_dek, wrap_dek, WrappedDEK};
use crate::error::CryptoError;
use crate::keys::MasterKey;

/// Write plaintext to an encrypted file.
///
/// Generates a new DEK per file, wraps it with the master key,
/// then encrypts the plaintext with the DEK. File format:
/// `[u32 WrappedDEK length][WrappedDEK bytes][nonce || ciphertext]`
pub fn write_encrypted_file(
    path: &Path,
    plaintext: &[u8],
    master_key: &MasterKey,
) -> Result<(), CryptoError> {
    let dek = generate_dek();
    let wrapped_dek = wrap_dek(&dek, master_key)?;
    let ciphertext = aes::encrypt(plaintext, &dek)?;

    let wrapped_bytes = wrapped_dek.as_bytes();
    let wrapped_len = wrapped_bytes.len() as u32;

    let mut output = Vec::with_capacity(4 + wrapped_bytes.len() + ciphertext.len());
    output.extend_from_slice(&wrapped_len.to_le_bytes());
    output.extend_from_slice(wrapped_bytes);
    output.extend_from_slice(&ciphertext);

    std::fs::write(path, &output)?;
    Ok(())
}

/// Read and decrypt an encrypted file.
///
/// Parses the file format, unwraps the DEK with the master key,
/// then decrypts the ciphertext.
pub fn read_encrypted_file(path: &Path, master_key: &MasterKey) -> Result<Vec<u8>, CryptoError> {
    let data = std::fs::read(path)?;

    if data.len() < 4 {
        return Err(CryptoError::InvalidData(
            "encrypted file too short for header".to_string(),
        ));
    }

    let wrapped_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    if data.len() < 4 + wrapped_len {
        return Err(CryptoError::InvalidData(
            "encrypted file too short for wrapped DEK".to_string(),
        ));
    }

    let wrapped_dek = WrappedDEK::from_bytes(data[4..4 + wrapped_len].to_vec());
    let ciphertext = &data[4 + wrapped_len..];

    let dek = unwrap_dek(&wrapped_dek, master_key)?;
    aes::decrypt(ciphertext, &dek)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_master_key() -> MasterKey {
        MasterKey::from_bytes([42u8; 32])
    }

    #[test]
    fn write_read_round_trip() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test.enc");
        let master = test_master_key();
        let plaintext = b"hello encrypted file";

        write_encrypted_file(&path, plaintext, &master).expect("write");
        let decrypted = read_encrypted_file(&path, &master).expect("read");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn read_fails_with_wrong_key() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test.enc");
        let master1 = MasterKey::from_bytes([1u8; 32]);
        let master2 = MasterKey::from_bytes([2u8; 32]);

        write_encrypted_file(&path, b"secret data", &master1).expect("write");
        let result = read_encrypted_file(&path, &master2);
        assert!(result.is_err());
    }

    #[test]
    fn read_fails_with_missing_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.enc");
        let master = test_master_key();

        let result = read_encrypted_file(&path, &master);
        assert!(result.is_err());
    }

    #[test]
    fn on_disk_file_is_not_plaintext() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test.enc");
        let master = test_master_key();
        let plaintext = b"this should not appear in the file";

        write_encrypted_file(&path, plaintext, &master).expect("write");

        let raw = std::fs::read(&path).expect("raw read");
        // The plaintext should NOT appear as a substring of the raw file
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(
            !raw_str.contains("this should not appear"),
            "plaintext found in encrypted file"
        );
    }

    #[test]
    fn write_read_empty_plaintext() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.enc");
        let master = test_master_key();

        write_encrypted_file(&path, b"", &master).expect("write empty");
        let decrypted = read_encrypted_file(&path, &master).expect("read empty");
        assert!(decrypted.is_empty());
    }
}
