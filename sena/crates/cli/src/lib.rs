//! CLI library components.

pub mod daemon_ipc;
pub mod error;
pub mod shell;

pub use error::CliError;
pub use shell::Shell;
