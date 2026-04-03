//! Single instance enforcement via IPC.
//!
//! On startup:
//! 1. Try to connect to the IPC endpoint (named pipe on Windows, Unix socket on macOS/Linux).
//! 2. If connection succeeds → another instance is running → return error.
//! 3. If connection fails → create the IPC server to claim the lock.
//!
//! The lock is automatically released when the `SingleInstanceGuard` is dropped (on shutdown).

use std::io::{self, ErrorKind};
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

/// Guard that holds the single-instance lock.
///
/// When dropped, the IPC endpoint is cleaned up.
pub struct SingleInstanceGuard {
    #[cfg(unix)]
    _listener: UnixListener,
    #[cfg(unix)]
    socket_path: PathBuf,
    #[cfg(windows)]
    _pipe_name: String,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // Clean up the Unix socket file.
            let _ = std::fs::remove_file(&self.socket_path);
            tracing::debug!("single instance lock released: {:?}", self.socket_path);
        }
        #[cfg(windows)]
        {
            // Named pipes on Windows are automatically cleaned up when closed.
            tracing::debug!("single instance lock released: {}", self._pipe_name);
        }
    }
}

impl SingleInstanceGuard {
    /// Create a test-only guard that doesn't actually enforce single-instance.
    ///
    /// This is used in unit tests where we need to construct a Runtime but don't
    /// want to interfere with the real IPC lock mechanism.
    #[cfg(test)]
    pub(crate) fn test_dummy() -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixListener;
            let socket_path = std::env::temp_dir()
                .join(format!("sena-test-{}.sock", std::process::id()));
            let _ = std::fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path)
                .expect("test dummy guard should always succeed");
            Self {
                _listener: listener,
                socket_path,
            }
        }
        #[cfg(windows)]
        {
            Self {
                _pipe_name: format!(r"\\.\pipe\sena_test_{}", std::process::id()),
            }
        }
    }
}

/// Attempt to acquire the single-instance lock.
///
/// Returns `Ok(guard)` if this is the only instance. The guard must be kept alive
/// for the duration of the process.
///
/// Returns `Err(AlreadyRunning)` if another instance is detected.
pub fn try_acquire_lock() -> Result<SingleInstanceGuard, SingleInstanceError> {
    #[cfg(unix)]
    {
        let socket_path = ipc_socket_path();
        
        // Try to connect first — if successful, another instance is running.
        if UnixStream::connect(&socket_path).is_ok() {
            return Err(SingleInstanceError::AlreadyRunning);
        }

        // Delete stale socket file if it exists (from unclean shutdown).
        let _ = std::fs::remove_file(&socket_path);

        // Create the Unix socket listener to claim the lock.
        let listener = UnixListener::bind(&socket_path)
            .map_err(|e| SingleInstanceError::LockFailed(e.to_string()))?;

        tracing::debug!("single instance lock acquired: {:?}", socket_path);

        Ok(SingleInstanceGuard {
            _listener: listener,
            socket_path,
        })
    }

    #[cfg(windows)]
    {
        use std::ptr;
        use winapi::um::fileapi::CreateFileW;
        use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
        use winapi::um::winbase::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
        use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ, GENERIC_WRITE};

        let pipe_name = r"\\.\pipe\sena_single_instance".to_string();
        let wide_name: Vec<u16> = OsStr::new(&pipe_name)
            .encode_wide()
            .chain(Some(0))
            .collect();

        // Try to open the existing pipe first (test if another instance is running).
        let test_handle = unsafe {
            CreateFileW(
                wide_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null_mut(),
                3, // OPEN_EXISTING
                0,
                ptr::null_mut(),
            )
        };

        if test_handle != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(test_handle) };
            return Err(SingleInstanceError::AlreadyRunning);
        }

        // Create the named pipe with FILE_FLAG_FIRST_PIPE_INSTANCE.
        // This flag ensures creation fails if the pipe already exists.
        let handle = unsafe {
            winapi::um::namedpipeapi::CreateNamedPipeW(
                wide_name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                0, // PIPE_TYPE_BYTE | PIPE_WAIT
                1, // max instances
                512,
                512,
                0,
                ptr::null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            let err = io::Error::last_os_error();
            if err.kind() == ErrorKind::PermissionDenied || err.raw_os_error() == Some(231) {
                // ERROR_PIPE_BUSY (231) means the pipe already exists.
                return Err(SingleInstanceError::AlreadyRunning);
            }
            return Err(SingleInstanceError::LockFailed(err.to_string()));
        }

        tracing::debug!("single instance lock acquired: {}", pipe_name);

        // CRITICAL: We must NOT close the handle here. Closing it would release the lock.
        // Instead, we intentionally leak the handle so it stays open for the process lifetime.
        // The OS will clean it up when the process exits.
        std::mem::forget(handle);

        Ok(SingleInstanceGuard {
            _pipe_name: pipe_name,
        })
    }
}

#[cfg(unix)]
fn ipc_socket_path() -> PathBuf {
    // Use a temp directory for the socket file. Use a fixed name per user.
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    std::env::temp_dir().join(format!("sena-{}.sock", user))
}

/// Non-destructive check: returns `true` if the Sena daemon is already running.
///
/// Does NOT acquire or modify the lock — safe to call from any mode.
pub fn is_daemon_running() -> bool {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;

        let pipe_name = r"\\.\pipe\sena_single_instance";
        let wide_name: Vec<u16> = OsStr::new(pipe_name)
            .encode_wide()
            .chain(Some(0))
            .collect();

        let handle = unsafe {
            winapi::um::fileapi::CreateFileW(
                wide_name.as_ptr(),
                winapi::um::winnt::GENERIC_READ,
                winapi::um::winnt::FILE_SHARE_READ | winapi::um::winnt::FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                3, // OPEN_EXISTING
                0,
                std::ptr::null_mut(),
            )
        };

        if handle != winapi::um::handleapi::INVALID_HANDLE_VALUE {
            unsafe { winapi::um::handleapi::CloseHandle(handle) };
            true
        } else {
            false
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        UnixStream::connect(ipc_socket_path()).is_ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SingleInstanceError {
    #[error("another instance of Sena is already running")]
    AlreadyRunning,

    #[error("failed to acquire single instance lock: {0}")]
    LockFailed(String),
}
