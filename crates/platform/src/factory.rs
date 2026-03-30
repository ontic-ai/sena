//! Platform adapter factory for creating OS-specific implementations.

use crate::adapter::PlatformAdapter;

/// Create the platform adapter for the current OS.
///
/// Returns a boxed trait object implementing PlatformAdapter
/// for the target operating system.
pub fn create_platform_adapter() -> Box<dyn PlatformAdapter> {
    #[cfg(target_os = "windows")]
    {
        Box::new(crate::windows::WindowsPlatform::new())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(crate::macos::MacOSPlatform::new())
    }

    #[cfg(target_os = "linux")]
    {
        Box::new(crate::linux::LinuxPlatform::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_creates_platform_adapter() {
        let adapter = create_platform_adapter();
        // Verify we can call trait methods
        let _ = adapter.active_window();
        let _ = adapter.clipboard_digest();
    }
}
