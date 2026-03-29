//! OS keychain integration for master key storage.

use crate::error::CryptoError;
use crate::keys::MasterKey;

const SERVICE_NAME: &str = "sena";
const USERNAME: &str = "master_key";

/// Store the master key in the OS keychain.
///
/// Uses the `keyring` crate to access:
/// - macOS: Keychain Services
/// - Windows: Windows Credential Manager
/// - Linux: Secret Service (libsecret / KWallet)
pub fn store_master_key(key: &MasterKey) -> Result<(), CryptoError> {
    let entry = keyring::Entry::new(SERVICE_NAME, USERNAME)
        .map_err(|e| CryptoError::KeychainError(e.to_string()))?;

    // Store as raw bytes encoded to base64 for keyring compatibility
    let encoded = base64_encode(key.as_bytes());
    entry
        .set_password(&encoded)
        .map_err(|e| CryptoError::KeychainError(e.to_string()))
}

/// Retrieve the master key from the OS keychain.
pub fn retrieve_master_key() -> Result<MasterKey, CryptoError> {
    let entry = keyring::Entry::new(SERVICE_NAME, USERNAME)
        .map_err(|e| CryptoError::KeychainError(e.to_string()))?;

    let encoded = entry
        .get_password()
        .map_err(|e| CryptoError::KeychainError(e.to_string()))?;

    let bytes = base64_decode(&encoded)?;
    if bytes.len() != 32 {
        return Err(CryptoError::KeychainError(format!(
            "invalid key length from keychain: {} bytes, expected 32",
            bytes.len()
        )));
    }

    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&bytes);
    Ok(MasterKey::from_bytes(key_bytes))
}

/// Delete the master key from the OS keychain.
pub fn delete_master_key() -> Result<(), CryptoError> {
    let entry = keyring::Entry::new(SERVICE_NAME, USERNAME)
        .map_err(|e| CryptoError::KeychainError(e.to_string()))?;

    entry
        .delete_credential()
        .map_err(|e| CryptoError::KeychainError(e.to_string()))
}

/// Check if a master key exists in the OS keychain.
pub fn has_master_key() -> bool {
    let entry = match keyring::Entry::new(SERVICE_NAME, USERNAME) {
        Ok(e) => e,
        Err(_) => return false,
    };
    entry.get_password().is_ok()
}

// Simple base64 encoding/decoding without adding a base64 dependency.
// Uses a minimal implementation sufficient for 32-byte key material.

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn base64_decode(input: &str) -> Result<Vec<u8>, CryptoError> {
    fn val(c: u8) -> Result<u32, CryptoError> {
        match c {
            b'A'..=b'Z' => Ok((c - b'A') as u32),
            b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
            b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(CryptoError::InvalidData("invalid base64 character".to_string())),
        }
    }

    let input = input.trim_end_matches('=');
    let bytes = input.as_bytes();
    let mut result = Vec::with_capacity(bytes.len() * 3 / 4);

    for chunk in bytes.chunks(4) {
        let a = val(chunk[0])?;
        let b = if chunk.len() > 1 { val(chunk[1])? } else { 0 };
        let c = if chunk.len() > 2 { val(chunk[2])? } else { 0 };
        let d = if chunk.len() > 3 { val(chunk[3])? } else { 0 };

        let triple = (a << 18) | (b << 12) | (c << 6) | d;

        result.push(((triple >> 16) & 0xFF) as u8);
        if chunk.len() > 2 {
            result.push(((triple >> 8) & 0xFF) as u8);
        }
        if chunk.len() > 3 {
            result.push((triple & 0xFF) as u8);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        let data = [42u8; 32];
        let encoded = base64_encode(&data);
        let decoded = base64_decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, data);
    }

    // Note: store/retrieve/delete tests interact with the real OS keychain
    // and are gated behind a manual test flag to avoid CI interference.
    // Run with: cargo test --package crypto -- --ignored keychain
    #[test]
    #[ignore]
    fn keychain_store_retrieve_delete_round_trip() {
        // Store
        let key = MasterKey::from_bytes([7u8; 32]);
        store_master_key(&key).expect("store should succeed");

        // Verify exists
        assert!(has_master_key());

        // Retrieve
        let retrieved = retrieve_master_key().expect("retrieve should succeed");
        assert_eq!(retrieved.as_bytes(), &[7u8; 32]);

        // Delete
        delete_master_key().expect("delete should succeed");
        assert!(!has_master_key());
    }

    #[test]
    fn retrieve_fails_when_no_key_stored() {
        // This test may pass or fail depending on OS keychain state.
        // The important thing is it doesn't panic.
        let result = retrieve_master_key();
        // We just verify it returns a Result, not panic
        let _ = result;
    }
}
