use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Master key — never written to disk, never logged.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MasterKey([REDACTED])")
    }
}

/// Data Encryption Key — ephemeral, derived from master key.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Dek([u8; 32]);

impl Dek {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Dek {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Dek([REDACTED])")
    }
}

/// Passphrase — zeroed immediately after use.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Passphrase(String);

impl Passphrase {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Passphrase([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_key_redacts_debug_output() {
        let key = MasterKey::from_bytes([42u8; 32]);
        let debug_str = format!("{:?}", key);
        assert_eq!(debug_str, "MasterKey([REDACTED])");
        assert!(!debug_str.contains("42"));
    }

    #[test]
    fn dek_redacts_debug_output() {
        let dek = Dek::from_bytes([99u8; 32]);
        let debug_str = format!("{:?}", dek);
        assert_eq!(debug_str, "Dek([REDACTED])");
        assert!(!debug_str.contains("99"));
    }

    #[test]
    fn passphrase_redacts_debug_output() {
        let passphrase = Passphrase::new("super_secret".to_string());
        let debug_str = format!("{:?}", passphrase);
        assert_eq!(debug_str, "Passphrase([REDACTED])");
        assert!(!debug_str.contains("super_secret"));
    }

    #[test]
    fn master_key_round_trip() {
        let original = [123u8; 32];
        let key = MasterKey::from_bytes(original);
        assert_eq!(key.as_bytes(), &original);
    }

    #[test]
    fn dek_round_trip() {
        let original = [77u8; 32];
        let dek = Dek::from_bytes(original);
        assert_eq!(dek.as_bytes(), &original);
    }

    #[test]
    fn passphrase_construction() {
        let p = Passphrase::new("test_passphrase".to_string());
        assert_eq!(p.as_str(), "test_passphrase");
    }
}
