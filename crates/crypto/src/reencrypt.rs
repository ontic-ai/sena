//! Atomic re-encryption when master key changes.

use std::path::Path;

use crate::error::CryptoError;
use crate::file::{read_encrypted_file, write_encrypted_file};
use crate::keys::MasterKey;

/// Re-encrypt a single file with a new master key.
///
/// Atomic: reads with old key, writes to temp file with new key,
/// then renames to original path. Either full success or no change.
pub fn re_encrypt_file(
    path: &Path,
    old_master_key: &MasterKey,
    new_master_key: &MasterKey,
) -> Result<(), CryptoError> {
    let plaintext = read_encrypted_file(path, old_master_key)?;

    // Write to a temporary file in the same directory for atomic rename
    let temp_path = path.with_extension("enc.tmp");
    write_encrypted_file(&temp_path, &plaintext, new_master_key)?;

    // Atomic rename
    std::fs::rename(&temp_path, path).map_err(|e| {
        // Attempt cleanup of temp file on rename failure
        let _ = std::fs::remove_file(&temp_path);
        CryptoError::IoError(e)
    })
}

/// Re-encrypt all matching files in a directory.
///
/// Returns the list of successfully re-encrypted file paths.
/// If any file fails, the operation stops and returns an error.
/// Files already re-encrypted are not rolled back (caller should
/// treat partial re-encryption as requiring recovery).
pub fn re_encrypt_directory(
    dir: &Path,
    old_master_key: &MasterKey,
    new_master_key: &MasterKey,
    extensions: &[&str],
) -> Result<Vec<std::path::PathBuf>, CryptoError> {
    let mut re_encrypted = Vec::new();

    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let matches = extensions
            .iter()
            .any(|ext| path.extension().and_then(|e| e.to_str()) == Some(*ext));

        if matches {
            re_encrypt_file(&path, old_master_key, new_master_key)?;
            re_encrypted.push(path);
        }
    }

    Ok(re_encrypted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn key_a() -> MasterKey {
        MasterKey::from_bytes([1u8; 32])
    }

    fn key_b() -> MasterKey {
        MasterKey::from_bytes([2u8; 32])
    }

    #[test]
    fn re_encrypt_file_readable_with_new_key_only() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("data.enc");
        let plaintext = b"sensitive data";

        write_encrypted_file(&path, plaintext, &key_a()).expect("initial write");

        re_encrypt_file(&path, &key_a(), &key_b()).expect("re-encrypt");

        // Readable with new key
        let decrypted = read_encrypted_file(&path, &key_b()).expect("read new key");
        assert_eq!(decrypted, plaintext);

        // Not readable with old key
        let result = read_encrypted_file(&path, &key_a());
        assert!(result.is_err());
    }

    #[test]
    fn re_encrypt_file_no_temp_file_left() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("data.enc");
        write_encrypted_file(&path, b"data", &key_a()).expect("write");

        re_encrypt_file(&path, &key_a(), &key_b()).expect("re-encrypt");

        let temp_path = path.with_extension("enc.tmp");
        assert!(!temp_path.exists(), "temp file should be cleaned up");
    }

    #[test]
    fn re_encrypt_directory_processes_matching_files() {
        let dir = tempdir().expect("tempdir");

        // Create some encrypted files
        write_encrypted_file(&dir.path().join("a.enc"), b"data-a", &key_a()).expect("write a");
        write_encrypted_file(&dir.path().join("b.enc"), b"data-b", &key_a()).expect("write b");
        write_encrypted_file(&dir.path().join("c.txt"), b"data-c", &key_a()).expect("write c");

        let results =
            re_encrypt_directory(dir.path(), &key_a(), &key_b(), &["enc"]).expect("re-encrypt dir");

        assert_eq!(results.len(), 2);

        // .enc files readable with new key
        let dec_a = read_encrypted_file(&dir.path().join("a.enc"), &key_b()).expect("read a");
        assert_eq!(dec_a, b"data-a");

        // .txt file still readable with old key (not re-encrypted)
        let dec_c = read_encrypted_file(&dir.path().join("c.txt"), &key_a()).expect("read c");
        assert_eq!(dec_c, b"data-c");
    }
}
