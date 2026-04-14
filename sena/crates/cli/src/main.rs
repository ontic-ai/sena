//! Sena CLI entry point — Windows-only IPC-first stub.

mod daemon_ipc;
mod error;
mod shell;

use error::CliError;
use shell::Shell;
use tracing::error;

#[tokio::main]
async fn main() -> Result<(), CliError> {
    // Create and run the shell
    match Shell::new().await {
        Ok(shell) => {
            if let Err(e) = shell.run().await {
                error!("Shell error: {}", e);
                return Err(e);
            }
        }
        Err(e) => {
            error!("Failed to initialize shell: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
