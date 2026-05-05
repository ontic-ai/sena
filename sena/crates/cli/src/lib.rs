//! CLI library components.

pub mod config_editor;
pub mod error;
pub mod onboarding;
pub mod shell;
pub mod theme;
pub mod transparency_format;

pub use error::CliError;
pub use shell::Shell;
