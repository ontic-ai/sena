pub(crate) const DEFAULT_TERMINAL_COLUMNS: i16 = 220;
pub(crate) const DEFAULT_TERMINAL_ROWS: i16 = 55;

pub(crate) fn try_resize_default_console() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if should_skip_default_resize() {
        return Ok(());
    }

    try_resize_current_console(DEFAULT_TERMINAL_COLUMNS, DEFAULT_TERMINAL_ROWS)
}

#[cfg(target_os = "windows")]
fn should_skip_default_resize() -> bool {
    should_skip_default_resize_for_env(
        std::env::var_os("WT_SESSION"),
        std::env::var_os("TERM_PROGRAM"),
    )
}

#[cfg(target_os = "windows")]
fn should_skip_default_resize_for_env(
    wt_session: Option<std::ffi::OsString>,
    term_program: Option<std::ffi::OsString>,
) -> bool {
    wt_session.is_some()
        || term_program
            .as_deref()
            .is_some_and(|value| value == std::ffi::OsStr::new("Windows_Terminal"))
}

#[cfg(target_os = "windows")]
pub(crate) fn try_resize_current_console(columns: i16, rows: i16) -> Result<(), String> {
    use std::io;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        COORD, GetStdHandle, SMALL_RECT, STD_OUTPUT_HANDLE, SetConsoleScreenBufferSize,
        SetConsoleWindowInfo,
    };

    if columns <= 0 || rows <= 0 {
        return Err("console dimensions must be positive".to_string());
    }

    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle.is_null() || std::ptr::eq(handle, INVALID_HANDLE_VALUE) {
            return Err("failed to acquire stdout console handle".to_string());
        }

        let buffer_size = COORD {
            X: columns,
            Y: rows,
        };
        if SetConsoleScreenBufferSize(handle, buffer_size) == 0 {
            return Err(io::Error::last_os_error().to_string());
        }

        let window = SMALL_RECT {
            Left: 0,
            Top: 0,
            Right: columns - 1,
            Bottom: rows - 1,
        };
        if SetConsoleWindowInfo(handle, 1, &window) == 0 {
            return Err(io::Error::last_os_error().to_string());
        }
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn try_resize_current_console(_columns: i16, _rows: i16) -> Result<(), String> {
    Ok(())
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::should_skip_default_resize_for_env;
    use std::ffi::OsString;

    #[test]
    fn skips_resize_for_windows_terminal_session() {
        assert!(should_skip_default_resize_for_env(
            Some(OsString::from("session-id")),
            None,
        ));
    }

    #[test]
    fn skips_resize_for_windows_terminal_term_program() {
        assert!(should_skip_default_resize_for_env(
            None,
            Some(OsString::from("Windows_Terminal")),
        ));
    }

    #[test]
    fn resizes_for_classic_console_hosts() {
        assert!(!should_skip_default_resize_for_env(None, None));
        assert!(!should_skip_default_resize_for_env(
            None,
            Some(OsString::from("vscode")),
        ));
    }
}