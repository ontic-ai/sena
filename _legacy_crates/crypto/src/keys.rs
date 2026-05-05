use zeroize::ZeroizeOnDrop;

/// Master key used for envelope encryption. Never stored on disk in plaintext.
/// Automatically zeroed from memory on drop.
#[derive(ZeroizeOnDrop)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// Create a MasterKey from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get a reference to the key bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MasterKey([REDACTED])")
    }
}

/// Data Encryption Key — unique per encrypted file.
/// Automatically zeroed from memory on drop.
#[derive(ZeroizeOnDrop)]
pub struct DEK([u8; 32]);

impl DEK {
    /// Create a DEK from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get a reference to the key bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for DEK {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DEK([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_key_debug_redacts() {
        let key = MasterKey::from_bytes([42u8; 32]);
        let debug_output = format!("{:?}", key);
        assert_eq!(debug_output, "MasterKey([REDACTED])");
        assert!(!debug_output.contains("42"));
    }

    #[test]
    fn dek_debug_redacts() {
        let key = DEK::from_bytes([99u8; 32]);
        let debug_output = format!("{:?}", key);
        assert_eq!(debug_output, "DEK([REDACTED])");
        assert!(!debug_output.contains("99"));
    }

    #[test]
    fn master_key_round_trip() {
        let original_bytes = [
            1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32,
        ];
        let key = MasterKey::from_bytes(original_bytes);
        assert_eq!(key.as_bytes(), &original_bytes);
    }

    #[test]
    fn dek_round_trip() {
        let original_bytes = [255u8; 32];
        let key = DEK::from_bytes(original_bytes);
        assert_eq!(key.as_bytes(), &original_bytes);
    }

    #[test]
    fn master_key_is_send_and_static() {
        // This is a compile-time check
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<MasterKey>();
    }

    #[test]
    fn dek_is_send_and_static() {
        // This is a compile-time check
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<DEK>();
    }
}
