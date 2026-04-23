//! Single-instance enforcement via file locking.
//!
//! Ensures only one Sena daemon instance runs at a time by acquiring an
//! exclusive lock on a lock file in the Sena config directory.
//!
//! ## Platform Behavior
//!
//! - **Windows**: Uses `LockFileEx` via fs2
//! - **macOS/Linux**: Uses `flock` via fs2
//!
//! ## Lifetime
//!
//! The guard must be held for the process lifetime. When dropped, the lock is
//! released and another instance can start.

use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Single-instance guard.
///
/// Holds an exclusive file lock. When dropped, the lock is released.
/// Must be kept alive for the process lifetime to prevent concurrent instances.
#[derive(Debug)]
pub struct InstanceGuard {
    #[allow(dead_code)] // Held for its Drop behavior
    file: File,
    lock_path: PathBuf,
}

impl InstanceGuard {
    /// Acquire the instance guard.
    ///
    /// Returns Ok(InstanceGuard) if this is the only instance.
    /// Returns Err(io::Error) if another instance is already running or if
    /// file operations fail.
    ///
    /// The lock file is created at `<sena_dir>/.sena.lock`.
    pub fn acquire(sena_dir: &Path) -> io::Result<Self> {
        let lock_path = sena_dir.join(".sena.lock");

        debug!(
            lock_path = %lock_path.display(),
            "Attempting to acquire instance lock"
        );

        // Open or create the lock file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;

        // Attempt to acquire exclusive lock (non-blocking)
        // This will fail immediately if another process holds the lock
        file.try_lock_exclusive()?;

        info!(
            lock_path = %lock_path.display(),
            "Instance lock acquired"
        );

        Ok(InstanceGuard { file, lock_path })
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        debug!(
            lock_path = %self.lock_path.display(),
            "Releasing instance lock"
        );
        // File lock is automatically released when the file is dropped.
        // Explicit unlock not strictly needed but good practice.
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn guard_acquired_successfully() {
        let dir = tempdir().unwrap();
        let guard = InstanceGuard::acquire(dir.path());
        assert!(guard.is_ok());
    }

    #[test]
    fn second_instance_fails_when_guard_held() {
        let dir = tempdir().unwrap();

        // First instance acquires guard
        let _guard1 = InstanceGuard::acquire(dir.path()).unwrap();

        // Second instance fails
        let guard2 = InstanceGuard::acquire(dir.path());
        assert!(
            guard2.is_err(),
            "second instance should fail when lock is held"
        );
    }

    #[test]
    fn guard_released_on_drop() {
        let dir = tempdir().unwrap();

        {
            let _guard1 = InstanceGuard::acquire(dir.path()).unwrap();
            // guard1 dropped here
        }

        // Second instance succeeds after first is dropped
        let guard2 = InstanceGuard::acquire(dir.path());
        assert!(guard2.is_ok());
    }

    #[test]
    fn lock_file_created_in_correct_location() {
        let dir = tempdir().unwrap();
        let expected_lock_path = dir.path().join(".sena.lock");

        let _guard = InstanceGuard::acquire(dir.path()).unwrap();

        assert!(expected_lock_path.exists());
        assert!(fs::metadata(&expected_lock_path).unwrap().is_file());
    }
}
