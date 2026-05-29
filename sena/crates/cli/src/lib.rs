//! CLI library components.

pub mod actors_tab;
mod commands;
pub mod diagnostics_tab;
pub mod logging;
pub mod resources_tab;
mod tab_chrome;
pub mod tabs;
mod terminal_window;

pub mod daemon_client;
pub mod config_editor;
pub mod error;
pub mod onboarding;
pub mod shell;
pub mod test_mode;
pub mod theme;
pub mod transparency_format;

pub use error::CliError;
pub use shell::Shell;
