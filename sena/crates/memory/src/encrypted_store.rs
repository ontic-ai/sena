//! Encrypted store wrapper for ech0 and Soul data.
//!
//! BONES stub: provides the type scaffolding for encrypted redb access.
//! Full encryption integration happens when the crypto crate's key
//! derivation path is wired to boot sequence step 2.

use std::path::{Path, PathBuf};
use tracing::debug;

/// Wraps an on-disk store path with an associated encryption context.
///
/// In the full implementation this will hold an AES-GCM cipher context derived
/// from the master key and will transparently encrypt/decrypt all reads and writes.
pub struct EncryptedStore {
    /// Path to the underlying redb file.
    path: PathBuf,
}

impl EncryptedStore {
    /// Create a new encrypted store handle for the given path.
    ///
    /// The store file is not created or opened until the first actual read/write.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        debug!(?path, "EncryptedStore: registered path (stub — not yet open)");
        Self { path }
    }

    /// Returns the configured store path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn new_stores_path() {
        let store = EncryptedStore::new(PathBuf::from("/tmp/test.redb"));
        assert_eq!(store.path(), Path::new("/tmp/test.redb"));
    }
}
