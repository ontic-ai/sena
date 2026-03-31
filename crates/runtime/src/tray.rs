//! System tray integration.
//!
//! The TrayManager spawns a dedicated thread to run the native OS event loop
//! required by tray-icon (AppKit on macOS, Win32 on Windows, libappindicator on Linux).
//!
//! All tray menu clicks are broadcast as `SystemEvent::TrayMenuClicked` on the bus.
//! If the tray is unavailable on a platform, Sena continues without it.

use std::sync::Arc;
use std::time::Duration;

use bus::{Event, EventBus, SystemEvent, TrayMenuItem};
use tray_icon::Icon;

/// Commands sent to the tray thread from the main process.
enum TrayCommand {
    /// Update the tray tooltip with a status message.
    UpdateStatus(String),
    /// Show a notification (currently just logged).
    ShowNotification(String),
    /// Shut down the tray thread cleanly.
    Shutdown,
}

/// Manages the system tray icon and menu.
pub struct TrayManager {
    /// Channel to send commands to the tray thread.
    command_tx: Option<std::sync::mpsc::Sender<TrayCommand>>,
    /// Handle to the tray thread (for join on shutdown).
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl TrayManager {
    /// Initialize the system tray.
    ///
    /// Spawns a dedicated thread that creates the tray icon and runs the native event loop.
    /// If tray initialization fails (e.g., missing system dependencies on Linux),
    /// emits `SystemEvent::TrayUnavailable` on the bus and returns a dormant manager.
    ///
    /// # Arguments
    /// - `bus`: Event bus for broadcasting tray events.
    /// - `runtime_handle`: Tokio runtime handle for async operations in the tray thread.
    pub fn new(bus: Arc<EventBus>, runtime_handle: tokio::runtime::Handle) -> Self {
        let (command_tx, command_rx) = std::sync::mpsc::channel::<TrayCommand>();

        // Spawn the tray thread.
        let thread_handle = std::thread::spawn(move || {
            if let Err(e) = run_tray_loop(bus.clone(), command_rx, runtime_handle.clone()) {
                // Initialization failed — emit TrayUnavailable.
                let reason = e.to_string();
                eprintln!("Tray unavailable: {}", reason);
                runtime_handle.block_on(async {
                    let _ = bus
                        .broadcast(Event::System(SystemEvent::TrayUnavailable {
                            reason: reason.clone(),
                        }))
                        .await;
                });
            }
        });

        TrayManager {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
        }
    }

    /// Update the tray icon tooltip with a status message.
    pub fn update_status(&self, text: &str) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(TrayCommand::UpdateStatus(text.to_string()));
        }
    }

    /// Show a notification (currently just logged, OS notifications are complex).
    pub fn show_notification(&self, text: &str) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(TrayCommand::ShowNotification(text.to_string()));
        }
    }

    /// Shut down the tray thread cleanly.
    pub fn shutdown(mut self) {
        if let Some(tx) = self.command_tx.take() {
            let _ = tx.send(TrayCommand::Shutdown);
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Error type for tray initialization.
#[derive(Debug, thiserror::Error)]
#[allow(clippy::enum_variant_names)]
enum TrayError {
    #[error("tray icon creation failed: {0}")]
    IconCreationFailed(String),

    #[error("menu creation failed: {0}")]
    MenuCreationFailed(String),

    #[error("tray init failed: {0}")]
    InitFailed(String),
}

// Load logo from compiled-in bytes — path relative to WORKSPACE ROOT
const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo.png");

/// Load the tray icon from the compiled-in logo PNG.
///
/// Returns an Icon suitable for use with tray-icon. Falls back to a green square
/// if decoding fails (should never happen with a valid PNG).
fn load_icon() -> Result<Icon, TrayError> {
    use image::ImageReader;
    use std::io::Cursor;

    let img = ImageReader::new(Cursor::new(LOGO_PNG))
        .with_guessed_format()
        .map_err(|e| TrayError::IconCreationFailed(format!("PNG read error: {}", e)))?
        .decode()
        .map_err(|e| TrayError::IconCreationFailed(format!("PNG decode error: {}", e)))?;

    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    Icon::from_rgba(rgba.into_raw(), width, height)
        .map_err(|e| TrayError::IconCreationFailed(e.to_string()))
}

/// Run the tray event loop in a dedicated thread.
///
/// This function creates the tray icon and menu, then polls for:
/// - Tray menu click events
/// - Commands from the main process
///
/// Runs until a Shutdown command is received.
fn run_tray_loop(
    bus: Arc<EventBus>,
    command_rx: std::sync::mpsc::Receiver<TrayCommand>,
    runtime_handle: tokio::runtime::Handle,
) -> Result<(), TrayError> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    // Load icon from assets/logo.png — compiled in at build time.
    // Falls back to a green square if decode fails.
    let icon = load_icon().unwrap_or_else(|e| {
        eprintln!("[tray] icon load failed ({}), using fallback", e);
        let fallback_rgba = [0u8, 128, 0, 255].repeat(16 * 16);
        Icon::from_rgba(fallback_rgba, 16, 16).expect("fallback icon always valid")
    });

    // Build the tray menu.
    let menu = Menu::new();
    let item_status = MenuItem::new("Show Status", true, None);
    let item_thought = MenuItem::new("Show Last Thought", true, None);
    let item_cli = MenuItem::new("Open CLI", true, None);
    let separator = PredefinedMenuItem::separator();
    let item_quit = MenuItem::new("Quit", true, None);

    menu.append(&item_status)
        .map_err(|e| TrayError::MenuCreationFailed(e.to_string()))?;
    menu.append(&item_thought)
        .map_err(|e| TrayError::MenuCreationFailed(e.to_string()))?;
    menu.append(&item_cli)
        .map_err(|e| TrayError::MenuCreationFailed(e.to_string()))?;
    menu.append(&separator)
        .map_err(|e| TrayError::MenuCreationFailed(e.to_string()))?;
    menu.append(&item_quit)
        .map_err(|e| TrayError::MenuCreationFailed(e.to_string()))?;

    // Create the tray icon.
    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Sena")
        .with_icon(icon)
        .build()
        .map_err(|e| TrayError::InitFailed(e.to_string()))?;

    // Tray initialized successfully — emit TrayReady.
    runtime_handle.block_on(async {
        let _ = bus.broadcast(Event::System(SystemEvent::TrayReady)).await;
    });

    // Event loop: poll for tray events and commands.
    let menu_channel = TrayIconEvent::receiver();

    loop {
        // Process tray menu events.
        if let Ok(event) = menu_channel.try_recv() {
            match event {
                TrayIconEvent::Click { .. } => {
                    // Main tray icon click — no action for now.
                }
                TrayIconEvent::Enter { .. } | TrayIconEvent::Leave { .. } => {
                    // Mouse hover events — ignored for now.
                }
                _ => {
                    // Other events — ignored.
                }
            }
        }

        // Check for menu item clicks using the menu item IDs.
        // tray-icon emits menu events separately from tray icon events.
        if let Ok(menu_event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            let menu_item = if menu_event.id == item_status.id() {
                Some(TrayMenuItem::ShowStatus)
            } else if menu_event.id == item_thought.id() {
                Some(TrayMenuItem::ShowLastThought)
            } else if menu_event.id == item_cli.id() {
                Some(TrayMenuItem::OpenCli)
            } else if menu_event.id == item_quit.id() {
                Some(TrayMenuItem::Quit)
            } else {
                None
            };

            if let Some(item) = menu_item {
                runtime_handle.block_on(async {
                    let _ = bus
                        .broadcast(Event::System(SystemEvent::TrayMenuClicked(item)))
                        .await;
                });
            }
        }

        // Process commands from main process.
        match command_rx.try_recv() {
            Ok(TrayCommand::UpdateStatus(text)) => {
                let _ = tray_icon.set_tooltip(Some(text));
            }
            Ok(TrayCommand::ShowNotification(text)) => {
                // OS notifications are complex — just log for now.
                eprintln!("[Tray Notification] {}", text);
            }
            Ok(TrayCommand::Shutdown) => {
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // No command — continue polling.
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Main process dropped the sender — shut down.
                break;
            }
        }

        // Reduce CPU usage.
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_manager_constructs_without_panic() {
        // This test verifies that TrayManager::new does not panic when called.
        // It may emit TrayUnavailable if tray-icon is not available on this platform.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let bus = Arc::new(EventBus::new());
        let handle = rt.handle().clone();

        let _manager = TrayManager::new(bus, handle);
        // Manager constructed successfully (whether tray is available or not).
    }

    #[test]
    fn tray_manager_shutdown_does_not_hang() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let bus = Arc::new(EventBus::new());
        let handle = rt.handle().clone();

        let manager = TrayManager::new(bus, handle);
        manager.shutdown(); // Should complete without hanging.
    }

    #[test]
    fn update_status_does_not_panic_on_dormant_manager() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let bus = Arc::new(EventBus::new());
        let handle = rt.handle().clone();

        let manager = TrayManager::new(bus, handle);
        manager.update_status("Test status");
        // Should not panic even if tray is unavailable.
    }
}
