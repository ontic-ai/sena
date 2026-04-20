//! System tray integration for the Sena daemon.
//!
//! Provides a main-thread tray loop with menu items for:
//! - Launch CLI
//! - Config Editor
//! - Open Models Folder
//! - Shutdown Sena
//!
//! Tooltip updates are received via std::sync::mpsc channel.
//!
//! # Phase 2 Tray Limitations
//!
//! - Icon loading from assets/logo.ico is not yet implemented (no .ico file exists in assets/).
//!   A magenta fallback icon (32x32 solid color) is used instead.
//! - ICO decoding is deferred to Phase 3+ when a proper .ico asset is available.
//! - On non-Windows platforms, tray is unavailable and returns an error.

use std::sync::mpsc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIconBuilder,
};

/// Tray menu item IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    LaunchCli,
    ConfigEditor,
    OpenModels,
    Shutdown,
}

/// Tray tooltip update message.
#[derive(Debug, Clone)]
pub struct TooltipUpdate {
    pub text: String,
}

/// Tray loop result.
#[derive(Debug)]
pub enum TrayLoopResult {
    Shutdown,
    Error(String),
}

/// Run the tray loop on the main thread.
///
/// This function blocks until shutdown is requested via menu or the tooltip channel closes.
///
/// # Arguments
///
/// * `tooltip_rx` - Receiver for tooltip update messages
/// * `action_tx` - Sender for tray action events to daemon task
///
/// # Platform
///
/// Windows only. On other platforms, returns immediately with an error.
#[cfg(target_os = "windows")]
pub fn run_tray_loop(
    tooltip_rx: mpsc::Receiver<TooltipUpdate>,
    action_tx: std::sync::mpsc::Sender<TrayAction>,
) -> TrayLoopResult {
    use std::time::Duration;

    // Load icon — fallback to magenta if assets/logo.ico is missing
    let icon = match load_icon() {
        Ok(icon) => icon,
        Err(_) => {
            // Note: ICO loading failure is expected in Phase 2 (no .ico file in assets/).
            // Using magenta fallback. This will be replaced when a proper .ico is added.
            match create_magenta_icon() {
                Ok(icon) => icon,
                Err(e) => {
                    // Fallback icon creation failed — cannot proceed without an icon.
                    return TrayLoopResult::Error(format!("Failed to create fallback icon: {}", e));
                }
            }
        }
    };

    // Build menu
    let menu = Menu::new();
    let launch_cli_item = MenuItem::new("Launch CLI", true, None);
    let config_editor_item = MenuItem::new("Config Editor", true, None);
    let open_models_item = MenuItem::new("Open Models Folder", true, None);
    let separator = PredefinedMenuItem::separator();
    let shutdown_item = MenuItem::new("Shutdown Sena", true, None);

    menu.append(&launch_cli_item).ok();
    menu.append(&config_editor_item).ok();
    menu.append(&open_models_item).ok();
    menu.append(&separator).ok();
    menu.append(&shutdown_item).ok();

    // Build tray icon
    let tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Sena — Initializing...")
        .with_icon(icon)
        .build()
    {
        Ok(tray) => tray,
        Err(e) => return TrayLoopResult::Error(format!("Failed to build tray icon: {}", e)),
    };

    // Get menu event receiver
    let menu_event_rx = MenuEvent::receiver();

    // Main event loop
    loop {
        // Check for menu events (non-blocking poll)
        while let Ok(event) = menu_event_rx.try_recv() {
            let action = if event.id == launch_cli_item.id() {
                Some(TrayAction::LaunchCli)
            } else if event.id == config_editor_item.id() {
                Some(TrayAction::ConfigEditor)
            } else if event.id == open_models_item.id() {
                Some(TrayAction::OpenModels)
            } else if event.id == shutdown_item.id() {
                Some(TrayAction::Shutdown)
            } else {
                None
            };

            if let Some(action) = action {
                if action == TrayAction::Shutdown {
                    return TrayLoopResult::Shutdown;
                }

                if action_tx.send(action).is_err() {
                    return TrayLoopResult::Error("Action channel closed".to_string());
                }
            }
        }

        // Check for tooltip updates (non-blocking)
        match tooltip_rx.try_recv() {
            Ok(update) => {
                tray.set_tooltip(Some(&update.text)).ok();
            }
            Err(mpsc::TryRecvError::Empty) => {
                // No tooltip update available
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                return TrayLoopResult::Error("Tooltip channel closed".to_string());
            }
        }

        // Windows message pump handling
        // Note: The tray-icon crate does NOT handle the Windows message pump internally.
        // We must explicitly pump messages in this loop to process tray events.
        #[cfg(target_os = "windows")]
        {
            pump_windows_messages();
        }

        // Sleep to avoid busy-waiting. 50ms provides reasonable responsiveness.
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "windows")]
fn pump_windows_messages() {
    // Pump Windows message queue to process tray menu clicks.
    // This is required for tray-icon to deliver menu events via MenuEvent::receiver().
    // Without this, right-click menu interactions on Windows are never delivered.
    unsafe {
        // Use raw FFI to pump Windows messages without blocking.
        #[allow(non_snake_case)]
        #[repr(C)]
        struct Msg {
            hwnd: *mut std::ffi::c_void,
            message: u32,
            wParam: usize,
            lParam: isize,
            time: u32,
            pt: Point,
        }
        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }

        #[link(name = "user32")]
        unsafe extern "system" {
            fn PeekMessageW(
                lpMsg: *mut Msg,
                hWnd: *mut std::ffi::c_void,
                wMsgFilterMin: u32,
                wMsgFilterMax: u32,
                wRemoveMsg: u32,
            ) -> i32;
            fn TranslateMessage(lpMsg: *const Msg) -> i32;
            fn DispatchMessageW(lpMsg: *const Msg) -> isize;
        }

        const PM_REMOVE: u32 = 0x0001;

        let mut msg: Msg = std::mem::zeroed();
        // PeekMessage returns non-zero if a message is available.
        // Process all available messages without blocking.
        loop {
            let has_message = PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE);
            if has_message == 0 {
                break; // No more messages
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(target_os = "macos")]
pub fn run_tray_loop(
    _tooltip_rx: mpsc::Receiver<TooltipUpdate>,
    _action_tx: std::sync::mpsc::Sender<TrayAction>,
) -> TrayLoopResult {
    TrayLoopResult::Error("Tray not supported on macOS (Windows only)".to_string())
}

#[cfg(target_os = "linux")]
pub fn run_tray_loop(
    _tooltip_rx: mpsc::Receiver<TooltipUpdate>,
    _action_tx: std::sync::mpsc::Sender<TrayAction>,
) -> TrayLoopResult {
    TrayLoopResult::Error("Tray not supported on Linux (Windows only)".to_string())
}

/// Load icon from assets/logo.ico.
///
/// # Phase 2 Limitation
///
/// assets/logo.ico does not exist in the current repository.
/// This function will fail and fall back to the magenta icon.
/// When a proper .ico file is added to assets/, this will work.
#[cfg(target_os = "windows")]
fn load_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    // Phase 2: Attempt to load from filesystem path relative to executable.
    // Phase 3+: Use include_bytes! for embedded asset when logo.ico is added.
    let icon_path = std::env::current_exe()?
        .parent()
        .ok_or("no parent directory")?
        .join("assets")
        .join("logo.ico");

    let icon_bytes = std::fs::read(icon_path)?;
    let icon = Icon::from_rgba(decode_ico(&icon_bytes)?, 32, 32)?;
    Ok(icon)
}

#[cfg(target_os = "macos")]
fn load_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    Err("Icon loading not supported on macOS".into())
}

#[cfg(target_os = "linux")]
fn load_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    Err("Icon loading not supported on Linux".into())
}

/// Decode ICO file to RGBA bytes.
///
/// # Phase 2 Limitation
///
/// ICO decoding is not implemented. Returns an error.
/// When needed, use the `ico` crate or similar for proper ICO parsing.
fn decode_ico(_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Err("ICO decoding not implemented — .ico asset not available in Phase 2".into())
}

/// Create a magenta fallback icon (32x32 solid magenta).
fn create_magenta_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for _ in 0..(32 * 32) {
        rgba.push(255); // R
        rgba.push(0); // G
        rgba.push(255); // B
        rgba.push(255); // A
    }

    Ok(Icon::from_rgba(rgba, 32, 32)?)
}
