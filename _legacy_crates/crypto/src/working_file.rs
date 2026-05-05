//! Helpers for encrypted-file-backed working copies.
//!
//! Several Sena subsystems (Soul, Memory) follow the same pattern:
//!
//! 1. **Open:** Decrypt a persistent `.enc` file to a temporary working copy.
//! 2. **Flush:** Encrypt the working copy back to the persistent path.
//! 3. **Close:** Flush + remove the temporary working copy.
//!
//! This module extracts those primitives so they are defined once.

use std::path::Path;

use crate::error::CryptoError;
use crate::keys::MasterKey;

/// Remove a stale working file/directory left over from a previous crash.
///
/// Returns `Ok(true)` if something was removed, `Ok(false)` if the path
/// did not exist.
pub fn clean_stale_working(path: &Path) -> Result<bool, CryptoError> {
    if !path.exists() {
        return Ok(false);
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(CryptoError::IoError)?;
    } else {
        std::fs::remove_file(path).map_err(CryptoError::IoError)?;
    }
    Ok(true)
}

/// Decrypt an encrypted file to a working path.
///
/// If `encrypted_path` does not exist, returns `Ok(false)` and the working
/// path is left untouched. If it does exist, the decrypted contents are
/// written to `working_path` and `Ok(true)` is returned.
pub fn decrypt_to_working(
    encrypted_path: &Path,
    working_path: &Path,
    master_key: &MasterKey,
) -> Result<bool, CryptoError> {
    if !encrypted_path.exists() {
        return Ok(false);
    }
    let plaintext = crate::file::read_encrypted_file(encrypted_path, master_key)?;
    std::fs::write(working_path, &plaintext).map_err(CryptoError::IoError)?;
    Ok(true)
}

/// Encrypt a working file back to the persistent encrypted path.
pub fn encrypt_from_working(
    working_path: &Path,
    encrypted_path: &Path,
    master_key: &MasterKey,
) -> Result<(), CryptoError> {
    let plaintext = std::fs::read(working_path).map_err(CryptoError::IoError)?;
    crate::file::write_encrypted_file(encrypted_path, &plaintext, master_key)
}

/// Brief sleep to let Windows release file locks before cleanup.
///
/// On non-Windows platforms this is a no-op. The 10 ms pause is a
/// pragmatic workaround for Windows mmap handle release timing.
pub fn wait_for_file_lock_release() {
    #[cfg(target_os = "windows")]
    std::thread::sleep(std::time::Duration::from_millis(10));
}

/// Clean up a working file or directory after encryption.
///
/// Calls [`wait_for_file_lock_release`] first, then removes the path.
pub fn cleanup_working(path: &Path) {
    wait_for_file_lock_release();
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_master_key() -> MasterKey {
        MasterKey::from_bytes([42u8; 32])
    }

    #[test]
    fn round_trip_decrypt_encrypt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let enc_path = dir.path().join("data.enc");
        let work_path = dir.path().join("data.working");

        let original = b"hello encrypted working file";

        // Write encrypted file via existing primitive
        crate::file::write_encrypted_file(&enc_path, original, &test_master_key())
            .expect("write");

        // Decrypt to working
        let existed = decrypt_to_working(&enc_path, &work_path, &test_master_key())
            .expect("decrypt_to_working");
        assert!(existed);
        assert_eq!(std::fs::read(&work_path).unwrap(), original);

        // Modify working file
        std::fs::write(&work_path, b"modified content").unwrap();

        // Encrypt back
        encrypt_from_working(&work_path, &enc_path, &test_master_key())
            .expect("encrypt_from_working");

        // Verify round-trip
        let recovered =
            crate::file::read_encrypted_file(&enc_path, &test_master_key()).expect("read back");
        assert_eq!(recovered, b"modified content");

        // Cleanup
        cleanup_working(&work_path);
        assert!(!work_path.exists());
    }

    #[test]
    fn decrypt_nonexistent_returns_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        let existed = decrypt_to_working(
            &dir.path().join("nope.enc"),
            &dir.path().join("nope.working"),
            &test_master_key(),
        )
        .expect("should succeed");
        assert!(!existed);
    }

    #[test]
    fn clean_stale_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stale.working");
        std::fs::write(&path, b"stale").unwrap();
        assert!(clean_stale_working(&path).expect("clean"));
        assert!(!path.exists());
    }

    #[test]
    fn clean_stale_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stale_dir");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("file.txt"), b"data").unwrap();
        assert!(clean_stale_working(&path).expect("clean"));
        assert!(!path.exists());
    }

    #[test]
    fn clean_nonexistent_returns_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!clean_stale_working(&dir.path().join("nope")).expect("clean"));
    }
}
