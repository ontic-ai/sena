pub(crate) const DEFAULT_TERMINAL_COLUMNS: i16 = 220;
pub(crate) const DEFAULT_TERMINAL_ROWS: i16 = 55;

pub(crate) fn try_resize_default_console() -> Result<(), String> {
    try_resize_current_console(DEFAULT_TERMINAL_COLUMNS, DEFAULT_TERMINAL_ROWS)
}

#[cfg(target_os = "windows")]
pub(crate) fn try_resize_current_console(columns: i16, rows: i16) -> Result<(), String> {
    use std::io;
    use std::ffi::c_void;
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
        if handle.is_null() || handle == INVALID_HANDLE_VALUE as *mut c_void {
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